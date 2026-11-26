//! A request read once from what its host was sent, for every use to read.

use std::sync::Arc;

use rail_model::body::{BodyBuffer, BodyRead, ByteBudget, Unread};
use rail_model::request::ObservedRequest;

/// A request as its host is sent it: its request line and declared length from its headers, then
/// its body, kept up to a cap within a budget. It keeps no header line: capture reads none.
pub struct RequestGatherer {
    authority: Vec<u8>,
    method: Vec<u8>,
    path: Vec<u8>,
    declared: Option<u64>,
    body: BodyBuffer,
    cap_bytes: usize,
}

/// How a host saw a body end, in its mode's terms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    Whole,
    /// Stopped where the host may have cut it.
    MayBeCut,
    /// The host's configuration sent none of it.
    NotSent,
}

// How the gathering ended: as the host saw it, or dropped before the host saw it end.
#[derive(Clone, Copy, PartialEq, Eq)]
enum End {
    Host(Ending),
    Dropped,
}

impl RequestGatherer {
    /// From the header lines as sent, pseudo-headers included: `:authority`, `:method` and `:path`
    /// are its request line, the first `content-length` its declared length.
    pub fn start<N, V>(
        lines: impl IntoIterator<Item = (N, V)>,
        cap_bytes: usize,
        budget: &Arc<ByteBudget>,
    ) -> Self
    where
        N: AsRef<[u8]>,
        V: AsRef<[u8]> + Into<Vec<u8>>,
    {
        let mut gatherer = Self {
            authority: Vec::new(),
            method: Vec::new(),
            path: Vec::new(),
            declared: None,
            body: BodyBuffer::new(cap_bytes, budget),
            cap_bytes,
        };
        let mut length_seen = false;
        for (name, value) in lines {
            let part = match name.as_ref() {
                b":authority" => &mut gatherer.authority,
                b":method" => &mut gatherer.method,
                b":path" => &mut gatherer.path,
                name if !length_seen && name.eq_ignore_ascii_case(b"content-length") => {
                    length_seen = true;
                    gatherer.declared = read_length(value.as_ref());
                    continue;
                }
                _ => continue,
            };
            if part.is_empty() {
                *part = value.into();
            }
        }
        gatherer
    }

    /// Keeps `body`, sent in one message, without a copy.
    pub fn adopt(&mut self, body: Vec<u8>) {
        self.body.adopt(body);
    }

    /// The request, its body read whole if it came whole within the cap and the budget, else
    /// unread: not sent, cut short (of its declared length, or where the host may have cut it), or
    /// past either.
    pub fn finish(self, ending: Ending) -> ObservedRequest {
        self.end(End::Host(ending))
    }

    /// The request of a host that stopped before the body ended: cut short.
    pub(crate) fn finish_dropped(self) -> ObservedRequest {
        self.end(End::Dropped)
    }

    fn end(self, end: End) -> ObservedRequest {
        let received_bytes = self.body.received_bytes();
        // A declared length below what was sent isn't the body's.
        let body_bytes = self.declared.unwrap_or(received_bytes).max(received_bytes);
        // A body declared empty is whole, sent or not.
        let unsent = end == End::Host(Ending::NotSent) && self.declared != Some(0);
        // A `Content-Length` tells a body cut short itself; without one, the host's ending does.
        let cut =
            end == End::Dropped || (end == End::Host(Ending::MayBeCut) && self.declared.is_none());
        let body = if unsent {
            BodyRead::Unread(Unread::NotSent)
        } else if body_bytes > self.cap_bytes as u64 {
            BodyRead::Unread(Unread::PastCap)
        } else if cut {
            BodyRead::Unread(Unread::CutShort)
        } else if received_bytes < body_bytes {
            let unread = if received_bytes == 0 {
                Unread::NotSent
            } else {
                Unread::CutShort
            };
            BodyRead::Unread(unread)
        } else {
            match self.body.finish() {
                Ok(body) => BodyRead::Whole(Arc::new(body)),
                Err(unkept) => BodyRead::Unread(unkept.into()),
            }
        };
        ObservedRequest::new(
            self.authority,
            self.method,
            self.path,
            Vec::new(),
            body,
            body_bytes,
        )
    }
}

// `Content-Length` as sent; a chunked body declares none.
fn read_length(value: &[u8]) -> Option<u64> {
    std::str::from_utf8(value).ok()?.trim().parse().ok()
}
