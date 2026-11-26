use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::body::{BodyRead, Unread};
use crate::endpoint::{Endpoint, Resolution};
use crate::request::ObservedRequest;
use crate::request_line::RequestLine;

/// What a condition reads about one call: its ticket's claims (`None` without a usable ticket),
/// and its request.
#[derive(Clone, Copy)]
pub struct Environment<'a> {
    pub(crate) ticket: Option<&'a Map<String, Value>>,
    request: Request<'a>,
}

#[derive(Clone, Copy)]
enum Request<'a> {
    Observed(&'a ObservedRequest),
    /// A call given as a policy reads it: no request line and no body.
    Sample {
        endpoint: &'a Endpoint,
        headers: &'a Headers,
    },
}

impl<'a> Environment<'a> {
    /// For `request`, every source read as a path first reads it.
    pub fn of(request: &'a ObservedRequest, ticket: Option<&'a Map<String, Value>>) -> Self {
        Self {
            ticket,
            request: Request::Observed(request),
        }
    }

    /// For judging a call against the policy bundle given what a policy reads: its ticket claims,
    /// its endpoint and its headers. Its request line reads absent, its body as not sent.
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
            Request::Observed(request) => Some(request.request_line()),
            Request::Sample { .. } => None,
        }
    }

    pub(crate) fn headers(&self) -> &'a Headers {
        match self.request {
            Request::Observed(request) => request.headers(),
            Request::Sample { headers, .. } => headers,
        }
    }

    /// `None` for a session message, which names none.
    pub(crate) fn endpoint(&self) -> Result<Option<&'a Endpoint>, Unread> {
        match self.request {
            Request::Observed(request) => match request.endpoint()? {
                Resolution::Judged(endpoint) => Ok(Some(endpoint)),
                Resolution::Discovery => Ok(None),
            },
            Request::Sample { endpoint, .. } => Ok(Some(endpoint)),
        }
    }

    pub(crate) fn body(&self) -> &'a BodyRead {
        match self.request {
            Request::Observed(request) => &request.body,
            Request::Sample { .. } => &BodyRead::Unread(Unread::NotSent),
        }
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
