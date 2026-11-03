use std::collections::HashMap;

use uuid::Uuid;

use crate::bundle::{Action, BindingMode, Bundle, EnforcementMode, Fallback};
use crate::environment::{Endpoint, Environment, ResolvedKey};
use crate::evaluate::{Budget, CompiledCondition, Refusal, compile_condition, evaluate_condition};

/// A bundle in the form calls are judged against.
#[derive(Debug)]
pub struct Judge {
    /// Enabled policies only, in evaluation order.
    chain: Vec<ChainPolicy>,
    /// The chain positions each bound endpoint allows, in order; none for `open`.
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

/// What the judge answers for one call. The caller acts on it per the enforcement mode.
#[derive(Debug, PartialEq)]
pub struct Outcome {
    /// The fallback that denies the call, if one does.
    pub fallback: Option<FallbackDenial>,
    /// The chain's verdict: `None` where a fallback denies at `enforce`, which skips the chain.
    pub chain: Option<ChainVerdict>,
}

#[derive(Debug, PartialEq)]
pub enum FallbackDenial {
    /// No usable ticket, under `ticket_fallback: block`.
    Ticket,
    /// No binding for the endpoint, under `binding_fallback: block`.
    Binding,
}

#[derive(Debug, PartialEq)]
pub struct ChainVerdict {
    pub decision: Decision,
    /// The `alert` policies that matched, in evaluation order. None when the call is refused.
    pub alerts: Vec<Uuid>,
}

#[derive(Debug, PartialEq)]
pub enum Decision {
    Allowed,
    /// By the first `block` policy that matched.
    Denied(Uuid),
    Refused(Refusal),
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
    let bindings = bundle
        .bindings
        .iter()
        .map(|binding| {
            // A bound policy that is disabled binds nothing.
            let allowed = match binding.mode {
                BindingMode::Open => Vec::new(),
                BindingMode::Gated => (0..chain.len())
                    .filter(|&position| binding.policy_ids.contains(&chain[position].id))
                    .collect(),
            };
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
    judge: &Judge,
    environment: &Environment,
    mode: EnforcementMode,
    budget: &mut Budget,
) -> Outcome {
    let enforcing = mode == EnforcementMode::Enforce;
    let mut fallback = None;
    if environment.ticket.is_none() && judge.ticket_fallback == Fallback::Block {
        fallback = Some(FallbackDenial::Ticket);
        if enforcing {
            return Outcome {
                fallback,
                chain: None,
            };
        }
    }
    // A message naming no tool by design isn't an unbound endpoint: it faces the whole chain.
    let (narrowed, unbound) = match environment.endpoint {
        Endpoint::Keyless => (None, false),
        Endpoint::Unrecognised => (None, true),
        Endpoint::Resolved(key) => {
            let narrowed = judge.bindings.get(key);
            (narrowed, narrowed.is_none())
        }
    };
    if unbound && judge.binding_fallback == Fallback::Block {
        fallback.get_or_insert(FallbackDenial::Binding);
        if enforcing {
            return Outcome {
                fallback,
                chain: None,
            };
        }
    }
    let chain = match narrowed {
        Some(positions) => walk(
            positions.iter().map(|&position| &judge.chain[position]),
            environment,
            budget,
        ),
        None => walk(judge.chain.iter(), environment, budget),
    };
    Outcome {
        fallback,
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
                    decision: Decision::Refused(refusal),
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
