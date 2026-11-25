//! `environments.json`: named environments, each built as its use builds it.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Map, Value};

use super::value::Bytes;
use crate::body::{BodyBuffer, BodyRead, ByteBudget, Unread};
use crate::capture::ObservedCall;
use crate::condition::{Environment, Headers};
use crate::endpoint::Endpoint;
use crate::request_line::RequestLine;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Environments {
    environments: BTreeMap<String, Spec>,
}

#[derive(Deserialize)]
#[serde(tag = "for", rename_all = "snake_case", deny_unknown_fields)]
enum Spec {
    Judging {
        ticket: Option<Map<String, Value>>,
        endpoint: EndpointSpec,
        headers: Vec<(String, String)>,
    },
    Capture {
        #[serde(flatten)]
        call: CallSpec,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum EndpointSpec {
    Keyless,
    Unrecognised,
    Resolved(String),
}

/// A call as a vector writes it: as the callout hands it over.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CallSpec {
    authority: Bytes,
    method: Bytes,
    path: Bytes,
    body: BodySpec,
    body_bytes: u64,
    /// The bytes every body and its tree share, where a case is about passing them.
    byte_budget: Option<usize>,
}

#[derive(Deserialize)]
#[serde(untagged, deny_unknown_fields)]
enum BodySpec {
    Unread { unread: UnreadSpec },
    Sent(Bytes),
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum UnreadSpec {
    PastCap,
    PastBudget,
    NotSent,
    CutShort,
}

impl CallSpec {
    pub(super) fn to_call(&self) -> Result<ObservedCall, String> {
        let body = match &self.body {
            BodySpec::Unread { unread } => BodyRead::Unread(match unread {
                UnreadSpec::PastCap => Unread::PastCap,
                UnreadSpec::PastBudget => Unread::PastBudget,
                UnreadSpec::NotSent => Unread::NotSent,
                UnreadSpec::CutShort => Unread::CutShort,
            }),
            BodySpec::Sent(bytes) => {
                let budget = ByteBudget::new(self.byte_budget.unwrap_or(usize::MAX));
                let mut buffer = BodyBuffer::new(usize::MAX, &budget);
                buffer.append(&bytes.to_bytes()?);
                match buffer.finish() {
                    Ok(body) => BodyRead::Whole(Arc::new(body)),
                    Err(why) => BodyRead::Unread(why.into()),
                }
            }
        };
        Ok(ObservedCall {
            authority: self.authority.to_bytes()?,
            method: self.method.to_bytes()?,
            path: self.path.to_bytes()?,
            body,
            body_bytes: self.body_bytes,
        })
    }
}

/// An environment's sources, owned, for an `Environment` to borrow.
pub(super) enum Sources {
    Judging {
        ticket: Option<Map<String, Value>>,
        endpoint: Endpoint,
        headers: Headers,
    },
    Capture {
        call: ObservedCall,
        request_line: RequestLine,
    },
}

impl Sources {
    pub(super) fn environment(&self) -> Environment<'_> {
        match self {
            Self::Judging {
                ticket,
                endpoint,
                headers,
            } => Environment::for_judging(ticket.as_ref(), endpoint, headers),
            Self::Capture { call, request_line } => {
                Environment::for_capture(request_line, &call.body)
            }
        }
    }

    /// The call, for an environment built for capture.
    pub(super) fn call(&self) -> Result<(&ObservedCall, &RequestLine), String> {
        match self {
            Self::Capture { call, request_line } => Ok((call, request_line)),
            Self::Judging { .. } => Err("an environment for judging, which holds no call".into()),
        }
    }
}

/// Every environment, by name.
pub(super) struct Named(BTreeMap<String, Spec>);

impl Named {
    pub(super) fn read(text: &str) -> Result<Self, String> {
        let file: Environments =
            serde_json::from_str(text).map_err(|e| format!("environments.json: {e}"))?;
        Ok(Self(file.environments))
    }

    pub(super) fn build(&self, name: &str) -> Result<Sources, String> {
        let spec = self
            .0
            .get(name)
            .ok_or_else(|| format!("no environment {name:?}"))?;
        Ok(match spec {
            Spec::Judging {
                ticket,
                endpoint,
                headers,
            } => Sources::Judging {
                ticket: ticket.clone(),
                endpoint: match endpoint {
                    EndpointSpec::Keyless => Endpoint::Keyless,
                    EndpointSpec::Unrecognised => Endpoint::Unrecognised,
                    EndpointSpec::Resolved(key) => Endpoint::Resolved(key.parse()?),
                },
                headers: Headers::from_lines(headers.iter().map(|(name, value)| (name, value))),
            },
            Spec::Capture { call } => {
                let call = call.to_call()?;
                let request_line = RequestLine::read(&call.authority, &call.method, &call.path);
                Sources::Capture { call, request_line }
            }
        })
    }
}
