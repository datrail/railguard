//! The Python binding of `rail-policy`.

use pyo3::pymodule;

#[pymodule]
mod _native {
    use std::collections::HashMap;

    use ed25519_dalek::VerifyingKey;
    use pyo3::exceptions::PyValueError;
    use pyo3::types::{PyBytes, PyBytesMethods};
    use pyo3::{Bound, IntoPyObject, PyResult, pyclass, pyfunction};
    use rail_policy::{
        Budget, Decision, Endpoint, EnforcementMode, Environment, Evaluation, FallbackDenial,
        Headers, Judge, JudgingMode, Lookup, Refusal, TicketState, TicketVerifier, Trace,
        compile_bundle as compile, judge_traced, parse_bundle, parse_ticket, sanitize,
    };
    use serde_json::{Map, Value, json};
    use uuid::Uuid;

    #[pymodule_export]
    const DEFAULT_STEP_LIMIT: u64 = rail_policy::DEFAULT_STEP_LIMIT;

    /// Why the bundle can't be used, or `None` when it can.
    #[pyfunction]
    fn validate_bundle(json: &str) -> Option<String> {
        parse_bundle(json)
            .err()
            .map(|unusable| unusable.to_string())
    }

    /// A bundle compiled once, to judge many samples against.
    #[pyclass(frozen, module = "datrail_policy")]
    struct CompiledBundle {
        judge: Judge,
        mode: EnforcementMode,
    }

    #[derive(IntoPyObject)]
    enum Compiled {
        Bundle(CompiledBundle),
        Unusable(String),
    }

    /// The compiled bundle, or why it can't be used.
    #[pyfunction]
    fn compile_bundle(json: &str) -> Compiled {
        match parse_bundle(json) {
            Ok(bundle) => Compiled::Bundle(CompiledBundle {
                judge: compile(&bundle),
                mode: bundle.enforcement.mode,
            }),
            Err(unusable) => Compiled::Unusable(unusable.to_string()),
        }
    }

    /// The result as JSON text, in the shape the Python dataclasses read.
    #[pyfunction]
    #[pyo3(signature = (bundle, *, endpoint_key, headers, claims, mode, step_limit))]
    fn judge_sample(
        bundle: &CompiledBundle,
        endpoint_key: &str,
        headers: Vec<(String, String)>,
        claims: Option<&str>,
        mode: Option<&str>,
        step_limit: u64,
    ) -> PyResult<String> {
        let mode = match mode {
            None => bundle.mode.judging_mode(),
            Some("observe") => Some(JudgingMode::Observe),
            Some("enforce") => Some(JudgingMode::Enforce),
            Some(other) => {
                return Err(PyValueError::new_err(format!(
                    "mode \"{}\": not observe or enforce",
                    sanitize(other)
                )));
            }
        };
        // An unreadable sample is reported first, whatever the mode.
        let result = read_sample(endpoint_key, claims).map(|(endpoint, claims)| {
            let Some(mode) = mode else {
                return json!({"kind": "not_judged", "reason": "mode_none"});
            };
            let headers = Headers::from_lines(headers);
            let environment = Environment {
                ticket: claims.as_ref(),
                endpoint: &endpoint,
                headers: &headers,
            };
            let mut recorder = Recorder(Vec::new());
            let outcome = judge_traced(
                &bundle.judge,
                &environment,
                mode,
                &mut Budget::new(step_limit),
                &mut recorder,
            );
            let fallbacks: Vec<&str> = outcome
                .fallbacks
                .iter()
                .map(|fallback| match fallback {
                    FallbackDenial::Ticket => "ticket",
                    FallbackDenial::Binding => "binding",
                })
                .collect();
            let chain = outcome.chain.map(|chain| {
                let (decision, policy, refusal) = match chain.decision {
                    Decision::Allowed => ("allowed", None, None),
                    Decision::Denied(policy) => ("denied", Some(policy), None),
                    Decision::Refused { policy, refusal } => {
                        ("refused", Some(policy), Some(format_refusal(refusal)))
                    }
                };
                json!({
                    "decision": decision,
                    "policy": policy,
                    "refusal": refusal,
                    "alerts": chain.alerts,
                })
            });
            json!({
                "kind": "judged",
                "mode": match mode {
                    JudgingMode::Observe => "observe",
                    JudgingMode::Enforce => "enforce",
                },
                "fallbacks": fallbacks,
                "chain": chain,
                "trace": recorder.0,
            })
        });
        let result = result
            .unwrap_or_else(|problem| json!({"kind": "unreadable_sample", "problem": problem}));
        Ok(result.to_string())
    }

    /// How an `x-rail` value classifies, as JSON text, with the claims when it's valid.
    #[pyfunction]
    #[pyo3(signature = (raw_ticket, *, now_in_seconds, keys, issuer, audience))]
    fn read_ticket(
        raw_ticket: Option<&str>,
        now_in_seconds: i64,
        keys: HashMap<String, Bound<'_, PyBytes>>,
        issuer: Option<String>,
        audience: Option<String>,
    ) -> PyResult<String> {
        let keys = keys
            .into_iter()
            .map(|(kid, key)| {
                let key = <[u8; 32]>::try_from(key.as_bytes())
                    .ok()
                    .and_then(|key| VerifyingKey::from_bytes(&key).ok());
                let problem = || format!("key \"{}\": not an Ed25519 public key", sanitize(&kid));
                let key = key.ok_or_else(|| PyValueError::new_err(problem()))?;
                Ok((kid, key))
            })
            .collect::<PyResult<_>>()?;
        let verifier = TicketVerifier {
            keys,
            issuer,
            audience,
        };
        let state = parse_ticket(raw_ticket.unwrap_or(""), &verifier, now_in_seconds);
        let (state, claims) = match state {
            TicketState::Absent => ("absent", None),
            TicketState::Undecodable => ("undecodable", None),
            TicketState::BadSignature => ("bad_signature", None),
            TicketState::Malformed => ("malformed", None),
            TicketState::WrongAudience => ("wrong_audience", None),
            TicketState::Expired => ("expired", None),
            TicketState::Valid(ticket) => ("valid", Some(ticket.claims)),
        };
        Ok(json!({"kind": "ticket_reading", "state": state, "claims": claims}).to_string())
    }

    /// The endpoint, and the claims of a usable ticket, or why the sample can't be read.
    fn read_sample(
        endpoint_key: &str,
        claims: Option<&str>,
    ) -> Result<(Endpoint, Option<Map<String, Value>>), String> {
        let endpoint = match endpoint_key {
            "keyless" => Endpoint::Keyless,
            "unrecognised" => Endpoint::Unrecognised,
            key => Endpoint::Resolved(key.parse().map_err(|_| {
                format!(
                    "endpoint key \"{}\": not <path>#<method>#<call>, keyless or unrecognised",
                    sanitize(key)
                )
            })?),
        };
        let claims = claims
            .map(serde_json::from_str::<Map<String, Value>>)
            .transpose()
            .map_err(|e| format!("claims: {}", sanitize(&e.to_string())))?;
        Ok((endpoint, claims))
    }

    fn format_refusal(refusal: Refusal) -> &'static str {
        match refusal {
            Refusal::Uninterpretable => "uninterpretable",
            Refusal::BudgetExceeded => "budget_exceeded",
        }
    }

    /// Each policy evaluated, as the trace's JSON.
    struct Recorder(Vec<Value>);

    impl Trace for Recorder {
        fn visit(&mut self, policy: Uuid, evaluation: &Evaluation) {
            let resolved = evaluation.resolved.map(|lookup| match lookup {
                Lookup::Absent => json!({"kind": "absent"}),
                Lookup::Str(text) => json!({"kind": "string", "value": text}),
                Lookup::Number(number) => json!({"kind": "number", "value": number}),
                Lookup::Bool(value) => json!({"kind": "bool", "value": value}),
                Lookup::List(items) => json!({"kind": "list", "value": items}),
                Lookup::Object => json!({"kind": "object"}),
            });
            let result = match evaluation.result {
                Ok(true) => "held",
                Ok(false) => "did_not_hold",
                Err(_) => "refused",
            };
            self.0.push(json!({
                "policy": policy,
                "resolved": resolved,
                "result": result,
            }));
        }
    }
}
