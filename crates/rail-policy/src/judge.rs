use std::collections::HashMap;

use serde::Serialize;
use uuid::Uuid;

use crate::bundle::{Action, BindingMode, Bundle, EnforcementMode, Fallback};
use crate::endpoint::{Endpoint, ResolvedKey};
use crate::environment::Environment;
use crate::evaluate::{
    Budget, CompiledCondition, Evaluation, Refusal, Verdict, compile_condition, evaluate_condition,
};

/// A bundle compiled for judging: built once per bundle, then used for every call.
#[derive(Debug)]
pub struct CompiledBundle {
    /// The enabled policies, in the order they're evaluated.
    chain: Vec<ChainPolicy>,
    /// For each bound endpoint, the indices in `chain` of the policies that apply to it.
    bindings: HashMap<ResolvedKey, Vec<usize>>,
    binding_fallback: Fallback,
    ticket_fallback: Fallback,
}

#[derive(Debug)]
struct ChainPolicy {
    id: Uuid,
    action: Action,
    condition: CompiledCondition,
}

/// How the caller acts on an outcome: `observe` only logs it, `enforce` acts on it. There's no
/// `none`, since at `none` the judge isn't called.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JudgingMode {
    Observe,
    Enforce,
}

impl EnforcementMode {
    /// The judging mode, or `None` at `none`, where calls are forwarded without judging.
    pub fn judging_mode(self) -> Option<JudgingMode> {
        match self {
            Self::None => None,
            Self::Observe => Some(JudgingMode::Observe),
            Self::Enforce => Some(JudgingMode::Enforce),
        }
    }
}

/// What the rules concluded for one call. The caller decides what to do with it.
#[derive(Debug, PartialEq)]
pub struct Outcome {
    /// The fallbacks that deny the call, ticket first. At `enforce` the first one ends the
    /// judgement, so there's at most one.
    pub fallback_denials: Vec<FallbackDenial>,
    /// What the policies concluded, or `None` if they weren't consulted: at `enforce`, after a
    /// fallback denied.
    pub chain: Option<ChainOutcome>,
}

/// A fallback that denies a call. No policy is named.
#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackDenial {
    /// The call has no usable ticket, and `ticket_fallback` is `block`.
    Ticket,
    /// The endpoint has no binding, and `binding_fallback` is `block`.
    Binding,
}

/// What the policies concluded.
#[derive(Debug, PartialEq)]
pub struct ChainOutcome {
    pub decision: Decision,
    /// The `alert` policies whose conditions held, in evaluation order. Empty when the call is
    /// refused.
    pub alerts: Vec<Uuid>,
}

/// How the walk through the policies ended.
#[derive(Debug, PartialEq)]
pub enum Decision {
    /// No `block` policy's condition held.
    Allowed,
    /// The first `block` policy whose condition held.
    Denied(Uuid),
    /// The walk stopped at this policy: its condition can't be read, or the step budget ran out.
    Refused { policy: Uuid, refusal: Refusal },
}

pub fn compile_bundle(bundle: &Bundle) -> CompiledBundle {
    // `policies` is expected already in evaluation order, as `parse_bundle` leaves it.
    let chain: Vec<ChainPolicy> = bundle
        .policies
        .iter()
        .filter(|policy| policy.enabled)
        .map(|policy| ChainPolicy {
            id: policy.id,
            action: policy.action,
            condition: compile_condition(&policy.condition),
        })
        .collect();
    let positions: HashMap<Uuid, usize> = chain
        .iter()
        .enumerate()
        .map(|(position, policy)| (policy.id, position))
        .collect();
    let bindings = bundle
        .bindings
        .iter()
        .map(|binding| {
            let mut allowed: Vec<usize> = match binding.mode {
                BindingMode::Open => Vec::new(),
                // A bound policy that is disabled binds nothing.
                BindingMode::Gated => binding
                    .policy_ids
                    .iter()
                    .filter_map(|id| positions.get(id).copied())
                    .collect(),
            };
            // A binding picks which policies apply, not their order, and may name one twice:
            // walk them in evaluation order, each once.
            allowed.sort_unstable();
            allowed.dedup();
            (binding.endpoint_key.resolved.clone(), allowed)
        })
        .collect();
    CompiledBundle {
        chain,
        bindings,
        binding_fallback: bundle.binding_fallback,
        ticket_fallback: bundle.ticket_fallback,
    }
}

/// Told of each policy the walk evaluates, in order, to explain an outcome.
pub trait Tracer {
    fn record(&mut self, policy: Uuid, evaluation: &Evaluation);
}

/// Records nothing, so judging with it costs nothing.
pub struct NoTracer;

impl Tracer for NoTracer {
    fn record(&mut self, _: Uuid, _: &Evaluation) {}
}

/// The ticket fallback, then the binding fallback, then the chain. At `enforce` a fallback's
/// denial ends the judgement; at `observe` the chain is walked anyway, to preview it.
pub fn judge(
    bundle: &CompiledBundle,
    environment: &Environment,
    mode: JudgingMode,
    budget: &mut Budget,
) -> Outcome {
    judge_traced(bundle, environment, mode, budget, &mut NoTracer)
}

/// `judge`, telling `tracer` of each policy evaluated.
pub fn judge_traced(
    bundle: &CompiledBundle,
    environment: &Environment,
    mode: JudgingMode,
    budget: &mut Budget,
    tracer: &mut impl Tracer,
) -> Outcome {
    let enforcing = mode == JudgingMode::Enforce;
    let mut fallback_denials = Vec::new();
    if environment.ticket.is_none() && bundle.ticket_fallback == Fallback::Block {
        fallback_denials.push(FallbackDenial::Ticket);
        if enforcing {
            return Outcome {
                fallback_denials,
                chain: None,
            };
        }
    }
    // A message naming no tool by design isn't an unbound endpoint: it faces the whole chain.
    let (narrowed, unbound) = match environment.endpoint {
        Endpoint::Keyless => (None, false),
        Endpoint::Unrecognised => (None, true),
        Endpoint::Resolved(key) => {
            let narrowed = bundle.bindings.get(key);
            (narrowed, narrowed.is_none())
        }
    };
    if unbound && bundle.binding_fallback == Fallback::Block {
        fallback_denials.push(FallbackDenial::Binding);
        if enforcing {
            return Outcome {
                fallback_denials,
                chain: None,
            };
        }
    }
    let chain = match narrowed {
        Some(positions) => walk(
            positions.iter().map(|&position| &bundle.chain[position]),
            environment,
            budget,
            tracer,
        ),
        None => walk(bundle.chain.iter(), environment, budget, tracer),
    };
    Outcome {
        fallback_denials,
        chain: Some(chain),
    }
}

/// The first `block` policy that matches denies; `alert` policies are noted and passed.
fn walk<'a>(
    policies: impl Iterator<Item = &'a ChainPolicy>,
    environment: &Environment,
    budget: &mut Budget,
    tracer: &mut impl Tracer,
) -> ChainOutcome {
    let mut alerts = Vec::new();
    for policy in policies {
        let evaluation = evaluate_condition(&policy.condition, environment, budget);
        tracer.record(policy.id, &evaluation);
        match evaluation.verdict {
            // A refused call reports nothing, its alerts included.
            Verdict::Refused(refusal) => {
                let decision = Decision::Refused {
                    policy: policy.id,
                    refusal,
                };
                return ChainOutcome {
                    decision,
                    alerts: Vec::new(),
                };
            }
            Verdict::DoesNotHold => {}
            Verdict::Holds if policy.action == Action::Alert => alerts.push(policy.id),
            Verdict::Holds => {
                let decision = Decision::Denied(policy.id);
                return ChainOutcome { decision, alerts };
            }
        }
    }
    ChainOutcome {
        decision: Decision::Allowed,
        alerts,
    }
}
