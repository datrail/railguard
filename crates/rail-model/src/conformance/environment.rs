//! `environments.json`: named environments, each built as its use builds it.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Map, Value};

use super::value::{Bytes, decode_unread, keep};
use super::{DEFAULT_BYTE_BUDGET, read_written};
use crate::body::BodyRead;
use crate::condition::{self, Environment, Headers};
use crate::endpoint::Endpoint;
use crate::request::ObservedRequest;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Environments {
    environments: BTreeMap<String, Spec>,
}

#[derive(Deserialize)]
#[serde(tag = "for", rename_all = "snake_case", deny_unknown_fields)]
enum Spec {
    Judging {
        /// `null` for no usable ticket.
        #[serde(deserialize_with = "read_written")]
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
    /// The bytes a body and its tree share, where a case is about passing them.
    byte_budget: Option<usize>,
}

#[derive(Deserialize)]
#[serde(untagged, deny_unknown_fields)]
enum BodySpec {
    /// Why it isn't read: `past_cap`, `past_budget`, `not_sent`, `cut_short`.
    Unread {
        unread: String,
    },
    /// `<N bytes>` standing for N bytes of `x`, as the capture design writes a body, padded with
    /// spaces, which JSON allows after its value, to `bytes`.
    Shorthand {
        text: String,
        bytes: Option<usize>,
    },
    Sent(Bytes),
}

impl CallSpec {
    pub(super) fn to_call(&self) -> Result<ObservedRequest, String> {
        let sent = |bytes: &[u8]| match keep(bytes, self.byte_budget.unwrap_or(DEFAULT_BYTE_BUDGET))
        {
            Ok(body) => BodyRead::Whole(Arc::new(body)),
            Err(why) => BodyRead::Unread(why),
        };
        let body = match &self.body {
            BodySpec::Unread { .. } if self.byte_budget.is_some() => {
                return Err("a byte_budget beside a body not read".into());
            }
            BodySpec::Unread { unread } => BodyRead::Unread(
                decode_unread(unread)
                    .ok_or_else(|| format!("not a reason a body is unread: {unread}"))?,
            ),
            BodySpec::Shorthand { text, bytes } => sent(&expand(text, *bytes)?),
            BodySpec::Sent(bytes) => sent(&bytes.to_bytes()?),
        };
        Ok(ObservedRequest::new(
            self.authority.to_bytes()?,
            self.method.to_bytes()?,
            self.path.to_bytes()?,
            Vec::new(),
            body,
            self.body_bytes,
        ))
    }
}

fn expand(text: &str, bytes: Option<usize>) -> Result<Vec<u8>, String> {
    let mut expanded = String::new();
    let mut rest = text;
    while let Some(at) = rest.find('<') {
        let (before, marker) = rest.split_at(at);
        expanded.push_str(before);
        let end = marker
            .find(" bytes>")
            .ok_or_else(|| format!("a < not starting <N bytes> in {text:?}"))?;
        let count: usize = marker[1..end]
            .parse()
            .map_err(|_| format!("not a count of bytes in {text:?}"))?;
        expanded.push_str(&"x".repeat(count));
        rest = &marker[end + " bytes>".len()..];
    }
    expanded.push_str(rest);
    let padding = match bytes {
        Some(bytes) => bytes
            .checked_sub(expanded.len())
            .ok_or_else(|| format!("{text:?} is past its {bytes} bytes"))?,
        None => 0,
    };
    Ok((expanded + &" ".repeat(padding)).into_bytes())
}

/// An environment's sources, owned.
pub(super) enum Sources {
    Judging {
        ticket: Option<Map<String, Value>>,
        endpoint: Endpoint,
        headers: Headers,
    },
    Capture {
        call: ObservedRequest,
    },
}

impl Sources {
    /// As an `Environment` borrows them, nothing read yet.
    pub(super) fn open(&self) -> Opened<'_> {
        match self {
            Self::Judging {
                ticket,
                endpoint,
                headers,
            } => Opened::Judging(Environment::for_judging(ticket.as_ref(), endpoint, headers)),
            Self::Capture { call } => Opened::Capture(condition::Sources::new(call)),
        }
    }
}

/// An environment's sources, opened for an `Environment` to borrow.
pub(super) enum Opened<'s> {
    Judging(Environment<'s>),
    Capture(condition::Sources<'s>),
}

impl Opened<'_> {
    pub(super) fn environment(&self) -> Environment<'_> {
        match self {
            Self::Judging(environment) => *environment,
            // Capture reads no ticket: the proxy strips the agent's.
            Self::Capture(sources) => Environment::of(sources, None),
        }
    }

    /// The call's sources, for an environment built for capture.
    pub(super) fn call(&self) -> Result<&condition::Sources<'_>, String> {
        match self {
            Self::Capture(sources) => Ok(sources),
            Self::Judging(_) => Err("an environment for judging, which holds no call".into()),
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
            Spec::Capture { call } => Sources::Capture {
                call: call.to_call()?,
            },
        })
    }
}
