use serde_json::{Map, Value};

use crate::body::mcp::McpReading;
use crate::body::{BodyRead, Unparsed, Unread};
use crate::endpoint::{Endpoint, Resolution};
use crate::request::{Headers, ObservedRequest};
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
    /// For `request` and its ticket's claims (`None` without a usable ticket).
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

    /// `None` past the budget.
    pub(crate) fn headers(&self) -> Option<&'a Headers> {
        match self.request {
            Request::Observed(request) => request.headers(),
            Request::Sample { headers, .. } => Some(headers),
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

    /// Whether its body is whole but its tree past the budget.
    pub(crate) fn is_tree_past_budget(&self) -> bool {
        matches!(self.body(), BodyRead::Whole(body)
            if *body.mcp() == McpReading::Unparsed(Unparsed::PastBudget))
    }

    pub(crate) fn body(&self) -> &'a BodyRead {
        match self.request {
            Request::Observed(request) => &request.body,
            Request::Sample { .. } => &BodyRead::Unread(Unread::NotSent),
        }
    }
}
