use std::collections::HashMap;

use uuid::Uuid;

use serde::Deserialize;

use crate::bundle::{Action, BindingMode, Bundle, EnforcementMode, Fallback};
use crate::endpoint::{Endpoint, ResolvedKey};
use crate::environment::Environment;
use crate::evaluate::{Budget, CompiledCondition, Refusal, compile_condition, evaluate_condition};

/// A bundle compiled for judging: built once per bundle, then used for every call.
#[derive(Debug)]
pub struct Judge {
    /// The enabled policies, in the order they're evaluated.
    chain: Vec<ChainPolicy>,
    /// For each bound endpoint, the index in `chain` of the policies that apply to it.
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
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
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
    pub fallbacks: Vec<FallbackDenial>,
    /// What the policies concluded, or `None` if they weren't consulted: at `enforce`, after a
    /// fallback denied.
    pub chain: Option<ChainVerdict>,
}

/// A fallback that denies a call. No policy is named.
#[derive(Debug, PartialEq)]
pub enum FallbackDenial {
    /// The call has no usable ticket, and `ticket_fallback` is `block`.
    Ticket,
    /// The endpoint has no binding, and `binding_fallback` is `block`.
    Binding,
}

/// What the policies concluded.
#[derive(Debug, PartialEq)]
pub struct ChainVerdict {
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

pub fn compile_bundle(bundle: &Bundle) -> Judge {
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
            allowed.sort_unstable();
            allowed.dedup();
            (binding.endpoint_key.resolved.clone(), allowed)
        })
        .collect();
    Judge {
        chain,
        bindings,
        binding_fallback: bundle.binding_fallback,
        ticket_fallback: bundle.ticket_fallback,
    }
}

/// The ticket fallback, then the binding fallback, then the chain. At `enforce` a fallback's
/// denial ends the judgement; at `observe` the chain is walked anyway, to preview it.
pub fn judge(
    compiled: &Judge,
    environment: &Environment,
    mode: JudgingMode,
    budget: &mut Budget,
) -> Outcome {
    let enforcing = mode == JudgingMode::Enforce;
    let mut fallbacks = Vec::new();
    if environment.ticket.is_none() && compiled.ticket_fallback == Fallback::Block {
        fallbacks.push(FallbackDenial::Ticket);
        if enforcing {
            return Outcome {
                fallbacks,
                chain: None,
            };
        }
    }
    // A message naming no tool by design isn't an unbound endpoint: it faces the whole chain.
    let (narrowed, unbound) = match environment.endpoint {
        Endpoint::Keyless => (None, false),
        Endpoint::Unrecognised => (None, true),
        Endpoint::Resolved(key) => {
            let narrowed = compiled.bindings.get(key);
            (narrowed, narrowed.is_none())
        }
    };
    if unbound && compiled.binding_fallback == Fallback::Block {
        fallbacks.push(FallbackDenial::Binding);
        if enforcing {
            return Outcome {
                fallbacks,
                chain: None,
            };
        }
    }
    let chain = match narrowed {
        Some(positions) => walk(
            positions.iter().map(|&position| &compiled.chain[position]),
            environment,
            budget,
        ),
        None => walk(compiled.chain.iter(), environment, budget),
    };
    Outcome {
        fallbacks,
        chain: Some(chain),
    }
}

/// The first `block` policy that matches denies; `alert` policies are noted and passed.
fn walk<'a>(
    policies: impl Iterator<Item = &'a ChainPolicy>,
    environment: &Environment,
    budget: &mut Budget,
) -> ChainVerdict {
    let mut alerts = Vec::new();
    for policy in policies {
        let decision = match evaluate_condition(&policy.condition, environment, budget) {
            // A refused call reports nothing, its alerts included.
            Err(refusal) => {
                return ChainVerdict {
                    decision: Decision::Refused {
                        policy: policy.id,
                        refusal,
                    },
                    alerts: Vec::new(),
                };
            }
            Ok(false) => continue,
            Ok(true) if policy.action == Action::Alert => {
                alerts.push(policy.id);
                continue;
            }
            Ok(true) => Decision::Denied(policy.id),
        };
        return ChainVerdict { decision, alerts };
    }
    ChainVerdict {
        decision: Decision::Allowed,
        alerts,
    }
}
