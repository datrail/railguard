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
        Budget, ChainVerdict, Decision, Endpoint, EnforcementMode, Environment, Evaluation,
        FallbackDenial, Headers, Judge, JudgingMode, Lookup, Refusal, TicketState, TicketVerifier,
        Trace, Verdict, compile_bundle as compile, judge_traced, parse_bundle, parse_ticket,
        sanitize,
    };
    use serde::Serialize;
    use serde_json::{Map, Value};
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
        let result = match (read_sample(endpoint_key, claims), mode) {
            (Err(problem), _) => SampleResult::UnreadableSample { problem },
            (Ok(_), None) => SampleResult::NotJudged {
                reason: "mode_none",
            },
            (Ok((endpoint, claims)), Some(mode)) => {
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
                SampleResult::Judged {
                    mode,
                    fallbacks: outcome.fallbacks,
                    chain: outcome.chain.map(Chain::from),
                    trace: recorder.0,
                }
            }
        };
        Ok(write_json(&result))
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
                    .and_then(|key| VerifyingKey::from_bytes(&key).ok())
                    .ok_or_else(|| {
                        let kid = sanitize(&kid);
                        PyValueError::new_err(format!("key \"{kid}\": not an Ed25519 public key"))
                    })?;
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
        Ok(write_json(&TicketReading { state, claims }))
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

    fn write_json(value: &impl Serialize) -> String {
        serde_json::to_string(value).expect("nothing here fails to serialize")
    }

    // The types below are the Python classes' shapes, which flatten Rust's enums into a tag field
    // and optional fields.

    #[derive(Serialize)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    enum SampleResult {
        Judged {
            mode: JudgingMode,
            fallbacks: Vec<FallbackDenial>,
            chain: Option<Chain>,
            trace: Vec<Step>,
        },
        NotJudged {
            reason: &'static str,
        },
        UnreadableSample {
            problem: String,
        },
    }

    #[derive(Serialize)]
    struct Chain {
        decision: &'static str,
        policy: Option<Uuid>,
        refusal: Option<Refusal>,
        alerts: Vec<Uuid>,
    }

    impl From<ChainVerdict> for Chain {
        fn from(chain: ChainVerdict) -> Self {
            let (decision, policy, refusal) = match chain.decision {
                Decision::Allowed => ("allowed", None, None),
                Decision::Denied(policy) => ("denied", Some(policy), None),
                Decision::Refused { policy, refusal } => ("refused", Some(policy), Some(refusal)),
            };
            Self {
                decision,
                policy,
                refusal,
                alerts: chain.alerts,
            }
        }
    }

    #[derive(Serialize)]
    struct Step {
        policy: Uuid,
        resolved: Option<Resolved>,
        verdict: &'static str,
        refusal: Option<Refusal>,
    }

    /// Owned, since a trace outlives the call's environment.
    #[derive(Serialize)]
    #[serde(tag = "kind", content = "value", rename_all = "snake_case")]
    enum Resolved {
        Absent,
        String(String),
        Number(f64),
        Bool(bool),
        List(Vec<Value>),
        Object,
    }

    impl From<Lookup<'_>> for Resolved {
        fn from(lookup: Lookup) -> Self {
            match lookup {
                Lookup::Absent => Self::Absent,
                Lookup::Str(text) => Self::String(text.to_string()),
                Lookup::Number(number) => Self::Number(number),
                Lookup::Bool(value) => Self::Bool(value),
                Lookup::List(items) => Self::List(items.to_vec()),
                Lookup::Object => Self::Object,
            }
        }
    }

    #[derive(Serialize)]
    struct TicketReading {
        state: &'static str,
        claims: Option<Map<String, Value>>,
    }

    struct Recorder(Vec<Step>);

    impl Trace for Recorder {
        fn visit(&mut self, policy: Uuid, evaluation: &Evaluation) {
            let (verdict, refusal) = match evaluation.verdict {
                Verdict::Holds => ("holds", None),
                Verdict::DoesNotHold => ("does_not_hold", None),
                Verdict::Refused(refusal) => ("refused", Some(refusal)),
            };
            self.0.push(Step {
                policy,
                resolved: evaluation.resolved.map(Resolved::from),
                verdict,
                refusal,
            });
        }
    }
}
