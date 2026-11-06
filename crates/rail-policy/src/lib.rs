//! The rules language and the judge.

mod bundle;
mod check;
mod condition;
mod endpoint;
mod environment;
mod evaluate;
mod judge;
mod sanitize;
mod ticket;

pub use bundle::{
    Action, Binding, BindingMode, Bundle, EndpointKey, Enforcement, EnforcementMode, Fallback,
    Policy, SchemaVersion, UnusableBundle, parse_bundle,
};
pub use check::{
    ConditionProblem, LocationSegment, OperandShape, ProblemCode, ProblemScope, TicketClaimsSchema,
    check_condition,
};
pub use condition::{Condition, Path, Predicate, RequestPath, Scalar, TicketPath};
pub use endpoint::{Endpoint, Resolution, ResolvedKey, resolve_endpoint};
pub use environment::{Environment, Headers};
pub use evaluate::{
    Budget, CompiledCondition, DEFAULT_STEP_LIMIT, Evaluation, Lookup, Refusal, Verdict,
    compile_condition, evaluate_condition,
};
pub use judge::{
    ChainOutcome, CompiledBundle, Decision, FallbackDenial, JudgingMode, NoTracer, Outcome, Tracer,
    compile_bundle, judge, judge_traced,
};
pub use sanitize::{escape_unquoted, sanitize, sanitize_quoted};
pub use ticket::{
    SignedTicketConfig, Ticket, TicketState, parse_base64_ticket, parse_jwt_ticket, parse_key_set,
    parse_ticket,
};
