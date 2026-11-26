use std::cell::OnceCell;
use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::body::{BodyRead, Unread};
use crate::endpoint::{Endpoint, Resolution};
use crate::request::ObservedRequest;
use crate::request_line::RequestLine;

/// What a condition reads about one call: its ticket's claims (`None` without a usable ticket),
/// and its request.
#[derive(Clone, Copy, Debug)]
pub struct Environment<'a> {
    pub(crate) ticket: Option<&'a Map<String, Value>>,
    request: Request<'a>,
}

#[derive(Clone, Copy, Debug)]
enum Request<'a> {
    Observed(&'a Sources<'a>),
    /// A sample judged against a bundle: it has no request line and no body.
    Sample {
        endpoint: &'a Endpoint,
        headers: &'a Headers,
    },
}

impl<'a> Environment<'a> {
    /// For a call `sources` reads, every source read as a path first reads it.
    pub fn of(sources: &'a Sources<'a>, ticket: Option<&'a Map<String, Value>>) -> Self {
        Self {
            ticket,
            request: Request::Observed(sources),
        }
    }

    /// For judging a sample call against the policy bundle, given what a policy reads: its ticket
    /// claims, its endpoint and its headers. A path into its request line or body reads absent.
    pub fn for_judging(
        ticket: Option<&'a Map<String, Value>>,
        endpoint: &'a Endpoint,
        headers: &'a Headers,
    ) -> Self {
        Self {
            ticket,
            request: Request::Sample { endpoint, headers },
        }
    }

    pub(crate) fn request_line(&self) -> Option<&'a RequestLine> {
        match self.request {
            Request::Observed(sources) => Some(sources.request_line()),
            Request::Sample { .. } => None,
        }
    }

    pub(crate) fn headers(&self) -> &'a Headers {
        match self.request {
            Request::Observed(sources) => sources.headers(),
            Request::Sample { headers, .. } => headers,
        }
    }

    /// `None` for a session message, which names none.
    pub(crate) fn endpoint(&self) -> Result<Option<&'a Endpoint>, Unread> {
        match self.request {
            Request::Observed(sources) => match sources.request.endpoint()? {
                Resolution::Judged(endpoint) => Ok(Some(endpoint)),
                Resolution::Discovery => Ok(None),
            },
            Request::Sample { endpoint, .. } => Ok(Some(endpoint)),
        }
    }

    pub(crate) fn body(&self) -> &'a BodyRead {
        match self.request {
            Request::Observed(sources) => &sources.request.body,
            Request::Sample { .. } => &NOT_SENT,
        }
    }
}

static NOT_SENT: BodyRead = BodyRead::Unread(Unread::NotSent);

/// What a call's conditions and records read of its request, each built the first time a path
/// reads it. One per call.
#[derive(Debug)]
pub struct Sources<'r> {
    pub(crate) request: &'r ObservedRequest,
    request_line: OnceCell<RequestLine>,
    headers: OnceCell<Headers>,
}

impl<'r> Sources<'r> {
    pub fn new(request: &'r ObservedRequest) -> Self {
        Self {
            request,
            request_line: OnceCell::new(),
            headers: OnceCell::new(),
        }
    }

    pub(crate) fn request_line(&self) -> &RequestLine {
        self.request_line.get_or_init(|| {
            let request = self.request;
            RequestLine::read(
                request.authority.as_bytes(),
                request.method.as_bytes(),
                request.path.as_bytes(),
            )
        })
    }

    fn headers(&self) -> &Headers {
        self.headers.get_or_init(|| {
            Headers::from_lines(
                self.request
                    .headers
                    .iter()
                    .map(|(name, value)| (name.as_str(), value.as_str())),
            )
        })
    }
}

/// A call's headers as conditions read them: names lowercased, every value split on commas.
/// Held as JSON strings, so a header's values read like a list in the ticket.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Headers(BTreeMap<String, Vec<Value>>);

impl Headers {
    /// From each line as sent, in order. Lines sharing a name pool their parts.
    pub fn from_lines<N, V>(lines: impl IntoIterator<Item = (N, V)>) -> Self
    where
        N: AsRef<str>,
        V: AsRef<str>,
    {
        let mut headers: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        for (name, value) in lines {
            let parts = headers.entry(name.as_ref().to_lowercase()).or_default();
            // Differs from Rail Center's `strip` only on control characters a header value can't hold.
            parts.extend(
                value
                    .as_ref()
                    .split(',')
                    .map(|part| Value::String(part.trim().to_string())),
            );
        }
        Self(headers)
    }

    pub(crate) fn get(&self, name: &str) -> Option<&[Value]> {
        self.0.get(name).map(Vec::as_slice)
    }
}
