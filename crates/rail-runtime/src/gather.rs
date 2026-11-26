//! A request read once from what its host was sent, for every use to read.

use std::sync::Arc;

use rail_model::body::{BodyBuffer, BodyRead, ByteBudget, Unread};
use rail_model::request::ObservedRequest;

// What every body a role keeps, and what's read from them, may take together.
pub(crate) const BODY_BUDGET_BYTES: usize = 256 * 1024 * 1024;

/// A request as its host is sent it: its request line, declared length and the header lines it
/// keeps from its headers, then its body, kept up to a cap within a budget.
pub struct RequestGatherer {
    authority: Vec<u8>,
    method: Vec<u8>,
    path: Vec<u8>,
    header_lines: Vec<(Vec<u8>, Vec<u8>)>,
    declared: Option<u64>,
    body: BodyBuffer,
    cap_bytes: usize,
    // A body adopted past the budget kept outside it, for a host that sends it back.
    keep_past_budget: bool,
}

/// How a host saw a body end, in its mode's terms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyEnding {
    Whole,
    /// Stopped where the host may have cut it.
    MayBeCut,
    /// Stopped before its end, as a stream ending first.
    CutShort,
    /// The host's configuration sent none of it.
    NotSent,
}

impl RequestGatherer {
    /// From the header lines as sent, pseudo-headers included: the first non-empty `:authority`,
    /// `:method` and `:path` are its request line, the first `content-length` its declared length.
    /// It keeps no header line: capture reads none.
    pub fn start<N, V>(
        lines: impl IntoIterator<Item = (N, V)>,
        cap_bytes: usize,
        budget: &Arc<ByteBudget>,
    ) -> Self
    where
        N: AsRef<[u8]>,
        V: AsRef<[u8]> + Into<Vec<u8>>,
    {
        Self::read(lines, false, cap_bytes, budget)
    }

    /// As `start`, keeping every header line but the pseudo-headers.
    pub(crate) fn keeping_lines<N, V>(
        lines: impl IntoIterator<Item = (N, V)>,
        cap_bytes: usize,
        budget: &Arc<ByteBudget>,
    ) -> Self
    where
        N: AsRef<[u8]>,
        V: AsRef<[u8]> + Into<Vec<u8>>,
    {
        Self::read(lines, true, cap_bytes, budget)
    }

    fn read<N, V>(
        lines: impl IntoIterator<Item = (N, V)>,
        keep: bool,
        cap_bytes: usize,
        budget: &Arc<ByteBudget>,
    ) -> Self
    where
        N: AsRef<[u8]>,
        V: AsRef<[u8]> + Into<Vec<u8>>,
    {
        let (mut authority, mut method, mut path) = (Vec::new(), Vec::new(), Vec::new());
        let (mut header_lines, mut declared, mut length_seen) = (Vec::new(), None, false);
        for (name, value) in lines {
            let name = name.as_ref();
            let part = match name {
                b":authority" => &mut authority,
                b":method" => &mut method,
                b":path" => &mut path,
                // The other pseudo-headers aren't header lines.
                [b':', ..] => continue,
                _ => {
                    if !length_seen && name.eq_ignore_ascii_case(b"content-length") {
                        length_seen = true;
                        declared = read_length(value.as_ref());
                    }
                    if keep {
                        header_lines.push((name.to_vec(), value.into()));
                    }
                    continue;
                }
            };
            if part.is_empty() {
                *part = value.into();
            }
        }
        Self {
            authority,
            method,
            path,
            header_lines,
            declared,
            body: BodyBuffer::new(cap_bytes, budget),
            cap_bytes,
            keep_past_budget: false,
        }
    }

    /// For a host whose `Content-Length` may not be its body's: the body then has no declared
    /// length.
    pub fn ignore_declared_length(&mut self) {
        self.declared = None;
    }

    pub(crate) fn keep_past_budget(&mut self) {
        self.keep_past_budget = true;
    }

    /// Keeps `body`, without a copy if it's the first; whether it's still kept, else past the cap
    /// or the budget, so a host refusing it can stop.
    pub fn adopt(&mut self, body: Vec<u8>) -> bool {
        if self.keep_past_budget {
            self.body.adopt_keeping(body);
        } else {
            self.body.adopt(body);
        }
        self.body.is_kept()
    }

    /// The request, its body read whole if it came whole within the cap and the budget, else
    /// unread: not sent, cut short (of its declared length, or where the host may have cut it), or
    /// past either.
    pub fn finish(self, ending: BodyEnding) -> ObservedRequest {
        let received_bytes = self.body.received_bytes();
        // A declared length below what was sent isn't the body's.
        let body_bytes = self.declared.unwrap_or(received_bytes).max(received_bytes);
        // A body declared empty is whole, sent or not.
        let unsent = ending == BodyEnding::NotSent && self.declared != Some(0);
        // A `Content-Length` tells a body that may be cut itself; without one, the host's ending does.
        let cut = ending == BodyEnding::CutShort
            || (ending == BodyEnding::MayBeCut && self.declared.is_none());
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
            self.header_lines,
            body,
            body_bytes,
        )
    }
}

// `Content-Length` as sent; a chunked body declares none.
fn read_length(value: &[u8]) -> Option<u64> {
    std::str::from_utf8(value).ok()?.trim().parse().ok()
}
