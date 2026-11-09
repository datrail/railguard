//! The gateway role: one call judged against the bundle held, with no IO but the denial it reports.

use std::collections::HashMap;

use rail_policy::{
    Budget, DEFAULT_STEP_LIMIT, Decision, Endpoint, Environment, Headers, JudgingMode, Outcome,
    Resolution, SignedTicketConfig, TicketState, judge, parse_ticket,
};

use crate::bundle_holder::HeldBundle;
use crate::denial::{Denial, DeniedBy, ticket_state};
use crate::key_set::KeySet;
use crate::message::resolve_body;
use crate::xrail::{XRAIL, XRAIL_STATUS};

/// One call, as the host received it.
#[derive(Clone, Debug)]
pub struct Call {
    /// Without the query, percent-decoded.
    pub path: String,
    /// Not read yet: the host passes anything but `POST` unjudged.
    pub method: String,
    pub body: String,
    /// Every header line as sent, `x-rail` and `x-rail-status` included.
    pub headers: Vec<(String, String)>,
}

/// How every call is judged, the same for each.
#[derive(Clone, Copy, Debug)]
pub struct UnitSettings {
    /// The steps one judgement may take before it's refused.
    pub step_limit: u64,
}

impl Default for UnitSettings {
    fn default() -> Self {
        Self {
            step_limit: DEFAULT_STEP_LIMIT,
        }
    }
}

/// What the host does with a call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Forward,
    /// Denied by a policy or a fallback: 403.
    Deny,
    /// The rules couldn't be applied to the call: 503.
    CannotApply,
}

impl Answer {
    /// The refusal's status and body, `None` for a call forwarded. The body names no policy, so
    /// a caller can't map the chain one attempt at a time.
    pub fn refusal(self) -> Option<(u16, &'static str)> {
        match self {
            Self::Forward => None,
            Self::Deny => Some((403, r#"{"error": "denied by policy"}"#)),
            Self::CannotApply => Some((503, r#"{"error": "policy ruleset cannot be applied"}"#)),
        }
    }
}

/// Judges `call` against `bundle`, a signed ticket against `keys` and the bundle's issuer and
/// audience, at `now_epoch_seconds`. A denial acted on goes to `report`, which mustn't wait.
pub fn judge_call(
    call: &Call,
    bundle: &HeldBundle,
    keys: Option<&KeySet>,
    now_epoch_seconds: i64,
    settings: &UnitSettings,
    report: &dyn Fn(&Denial),
) -> Answer {
    let endpoint = match resolve_body(&call.path, &call.body) {
        Resolution::Discovery => {
            tracing::info!("forwarded a session message, unjudged");
            return Answer::Forward;
        }
        Resolution::Judged(endpoint) => endpoint,
    };
    let named = name(&endpoint);
    let Some(mode) = bundle.posture.mode.judging_mode() else {
        tracing::info!(endpoint = named, "forwarded unjudged, at enforcement none");
        return Answer::Forward;
    };
    let ticket = read_ticket(call, bundle, keys, now_epoch_seconds);
    let headers = Headers::from_lines(call.headers.iter().map(|(name, value)| (name, value)));
    let environment = Environment {
        ticket: match &ticket {
            TicketState::Valid(ticket) => Some(&ticket.claims),
            _ => None,
        },
        endpoint: &endpoint,
        headers: &headers,
    };
    let outcome = judge(
        &bundle.compiled,
        &environment,
        mode,
        &mut Budget::new(settings.step_limit),
    );
    let ticket_named = ticket_state(&ticket);
    log_outcome(&outcome, mode, named, ticket_named);
    if mode == JudgingMode::Observe {
        return Answer::Forward;
    }
    if let Some(denied_by) = DeniedBy::of(&outcome) {
        report(&Denial {
            denied_by,
            endpoint: &endpoint,
            ticket: &ticket,
            claimed_status: one_value(call, XRAIL_STATUS),
        });
        return Answer::Deny;
    }
    match outcome.chain.map(|chain| chain.decision) {
        Some(Decision::Refused { .. }) => Answer::CannotApply,
        _ => Answer::Forward,
    }
}

// Every `x-rail` value: none is absent, and two are refused, since hosts collapse them differently.
fn read_ticket(
    call: &Call,
    bundle: &HeldBundle,
    keys: Option<&KeySet>,
    now_epoch_seconds: i64,
) -> TicketState {
    let mut values = values(call, XRAIL);
    let value = match (values.next(), values.next()) {
        (None, _) => return TicketState::Absent,
        (Some(value), None) => value,
        (Some(_), Some(_)) => return TicketState::Undecodable,
    };
    let no_keys = HashMap::new();
    let config = SignedTicketConfig {
        keys: keys.map_or(&no_keys, |set| &set.keys),
        issuer: bundle.ticket_issuer.as_deref(),
        audience: bundle.ticket_audience.as_deref(),
    };
    parse_ticket(value, &config, now_epoch_seconds)
}

fn values<'a>(call: &'a Call, header: &'a str) -> impl Iterator<Item = &'a str> {
    call.headers
        .iter()
        .filter(move |(name, _)| name.eq_ignore_ascii_case(header))
        .map(|(_, value)| value.as_str())
}

// A header's value if it was sent once: two claims aren't a claim.
fn one_value<'a>(call: &'a Call, header: &'a str) -> Option<&'a str> {
    let mut values = values(call, header);
    match (values.next(), values.next()) {
        (Some(value), None) => Some(value),
        _ => None,
    }
}

fn name(endpoint: &Endpoint) -> &str {
    match endpoint {
        Endpoint::Resolved(key) => key.as_str(),
        Endpoint::Keyless => "keyless",
        Endpoint::Unrecognised => "unrecognised",
    }
}

// What was concluded, and at `observe` what `enforce` would have done.
fn log_outcome(outcome: &Outcome, mode: JudgingMode, endpoint: &str, ticket: &str) {
    let observing = mode == JudgingMode::Observe;
    for fallback in &outcome.fallback_denials {
        if observing {
            tracing::warn!(
                endpoint,
                ticket,
                ?fallback,
                "would deny, by a fallback; forwarded, at enforcement observe"
            );
        } else {
            tracing::warn!(
                endpoint,
                ticket,
                ?fallback,
                "denied by a fallback, naming no policy"
            );
        }
    }
    let Some(chain) = &outcome.chain else {
        return;
    };
    for policy in &chain.alerts {
        tracing::warn!(endpoint, %policy, "a policy alerts");
    }
    match chain.decision {
        Decision::Allowed if outcome.fallback_denials.is_empty() => {
            tracing::info!(endpoint, ticket, "allowed");
        }
        Decision::Allowed => {}
        Decision::Denied(policy) if observing => tracing::warn!(
            endpoint,
            ticket,
            %policy,
            "would deny, by a policy; forwarded, at enforcement observe"
        ),
        Decision::Denied(policy) => tracing::warn!(endpoint, ticket, %policy, "denied by a policy"),
        Decision::Refused { policy, refusal } => tracing::error!(
            endpoint,
            %policy,
            ?refusal,
            refused = !observing,
            "the policy can't be applied to the call; Rail Center and this gateway may have drifted"
        ),
    }
}
