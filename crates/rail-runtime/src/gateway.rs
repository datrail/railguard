//! The gateway role: one call judged against the bundle held, with no IO but the denial it reports.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rail_model::body::mcp::McpReading;
use rail_model::body::{BodyRead, Unparsed};
use rail_model::condition::{Budget, DEFAULT_STEP_LIMIT, Environment, Refusal};
use rail_model::endpoint::{Endpoint, Resolution};
use rail_model::policy::{
    Decision, DecodedTicket, JudgingMode, Outcome, SignedTicketConfig, TicketState, decode_ticket,
    judge,
};
use rail_model::request::ObservedRequest;

use crate::bundle_holder::HeldBundle;
use crate::denial::{Denial, DeniedBy, ticket_state};
use crate::gateway_call::answer_unjudged_by;
use crate::key_set::KeySet;
use crate::xrail::{XRAIL, XRAIL_STATUS};

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
    /// Passed on to the upstream.
    Forward,
    /// Denied by a policy or a fallback: 403.
    Deny,
    /// The rules couldn't be applied to the call: 503.
    CannotApply,
    /// The body is past what's judged, or cut short: 413.
    TooLarge,
}

impl Answer {
    /// The refusal's status and body, `None` for a call forwarded. The body names no policy, so
    /// a caller can't map the chain one attempt at a time.
    pub fn refusal(self) -> Option<(u16, &'static str)> {
        match self {
            Self::Forward => None,
            Self::Deny => Some((403, r#"{"error": "denied by policy"}"#)),
            Self::CannotApply => Some((503, r#"{"error": "policy ruleset cannot be applied"}"#)),
            Self::TooLarge => Some((413, r#"{"error": "request body too large"}"#)),
        }
    }
}

/// One call's judgement, with everything it reads: gathered where the call arrives, run where
/// the work can take its time.
pub(crate) struct Unit {
    pub(crate) call: Arc<ObservedRequest>,
    /// The call's `x-rail`, decoded once where it arrived.
    pub(crate) ticket: DecodedTicket,
    pub(crate) bundle: Arc<HeldBundle>,
    /// Verifies a signed ticket, with the bundle's issuer and audience. None held reads
    /// `bad_signature`.
    pub(crate) keys: Option<Arc<KeySet>>,
    pub(crate) now_epoch_seconds: i64,
    pub(crate) settings: UnitSettings,
    /// Set once nobody waits for the answer: the judgement ends at its next step, reporting nothing.
    pub(crate) stop: Arc<AtomicBool>,
}

/// Judges `unit`'s call against its bundle. A denial acted on goes to `report`, which mustn't
/// wait.
pub(crate) fn judge_call(unit: Unit, report: &dyn Fn(&Denial)) -> Answer {
    let Unit {
        call,
        ticket,
        bundle,
        keys,
        now_epoch_seconds,
        settings,
        stop,
    } = unit;
    let unjudged = |why: &str| {
        let answer = answer_unjudged_by(&bundle);
        tracing::error!(?answer, "{why}; the call went unjudged");
        answer
    };
    // Checked first: its endpoint would read the call as naming no tool.
    if let BodyRead::Whole(body) = call.body()
        && *body.mcp() == McpReading::Unparsed(Unparsed::PastBudget)
    {
        return unjudged("its body's tree is past the memory budget");
    }
    let endpoint = match call.endpoint() {
        Ok(Resolution::Discovery) => {
            tracing::debug!("forwarded a session message, unjudged");
            return Answer::Forward;
        }
        Ok(Resolution::Judged(endpoint)) => endpoint,
        // Only a body read whole is judged.
        Err(_) => return Answer::CannotApply,
    };
    let named = name(endpoint);
    let Some(mode) = bundle.posture.mode.judging_mode() else {
        tracing::info!(endpoint = named, "forwarded unjudged, at enforcement none");
        return Answer::Forward;
    };
    let no_keys = HashMap::new();
    let config = SignedTicketConfig {
        keys: keys.as_ref().map_or(&no_keys, |set| &set.keys),
        issuer: bundle.ticket_issuer.as_deref(),
        audience: bundle.ticket_audience.as_deref(),
    };
    let ticket = ticket.verify(&config, now_epoch_seconds);
    let environment = Environment::of(
        &call,
        match &ticket {
            TicketState::Valid(ticket) => Some(&ticket.claims),
            _ => None,
        },
    );
    let mut budget = Budget::new(settings.step_limit).stopped_by(Arc::clone(&stop));
    let outcome = judge(&bundle.compiled, &environment, mode, &mut budget);
    // Claims the answer, unless it was given without this judgement: then nothing is acted on.
    if stop.swap(true, Ordering::AcqRel) {
        return Answer::Forward;
    }
    if let Some(Decision::Refused {
        refusal: Refusal::PastBudget,
        ..
    }) = outcome.chain.as_ref().map(|chain| &chain.decision)
    {
        return unjudged("its headers are past the memory budget");
    }
    log_outcome(&outcome, mode, named, ticket_state(&ticket));
    if mode == JudgingMode::Observe {
        return Answer::Forward;
    }
    if let Some(denied_by) = DeniedBy::of(&outcome) {
        report(&Denial {
            denied_by,
            endpoint,
            ticket: &ticket,
            claimed_status: one_value(&call, XRAIL_STATUS),
        });
        return Answer::Deny;
    }
    match outcome.chain.map(|chain| chain.decision) {
        Some(Decision::Refused { .. }) => Answer::CannotApply,
        _ => Answer::Forward,
    }
}

/// The call's `x-rail`, decoded: none is absent, and two are undecodable, since hosts collapse
/// repeated headers differently.
pub(crate) fn decode_x_rail(call: &ObservedRequest) -> DecodedTicket {
    let mut values = values(call, XRAIL);
    match (values.next(), values.next()) {
        (None, _) => DecodedTicket::Read(TicketState::Absent),
        (Some(value), None) => decode_ticket(value),
        (Some(_), Some(_)) => DecodedTicket::Read(TicketState::Undecodable),
    }
}

fn values<'a>(call: &'a ObservedRequest, header: &'a str) -> impl Iterator<Item = &'a str> {
    call.header_lines()
        .filter(move |(name, _)| name.eq_ignore_ascii_case(header))
        .map(|(_, value)| value)
}

// A header's value if it was sent once: two claims aren't a claim.
fn one_value<'a>(call: &'a ObservedRequest, header: &'a str) -> Option<&'a str> {
    let mut values = values(call, header);
    match (values.next(), values.next()) {
        (Some(value), None) => Some(value),
        _ => None,
    }
}

fn name(endpoint: &Endpoint) -> &str {
    match endpoint {
        Endpoint::Resolved(key) => key.as_str(),
        Endpoint::Keyless | Endpoint::Unrecognised => endpoint.kind(),
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
