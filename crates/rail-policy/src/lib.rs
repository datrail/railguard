//! The rules language and the judge.

mod bundle;
mod condition;
mod environment;
mod evaluate;
mod judge;
mod schema;
mod ticket;

pub use bundle::{
    Action, Binding, BindingMode, Bundle, EndpointKey, Enforcement, EnforcementMode, Fallback,
    Policy, SchemaVersion, UnusableBundle, parse_bundle,
};
pub use condition::{Condition, Path, Predicate, Scalar};
pub use environment::{Endpoint, Environment, Headers, Resolution, ResolvedKey, resolve_endpoint};
pub use evaluate::{
    Budget, CompiledCondition, DEFAULT_STEP_LIMIT, Refusal, compile_condition, evaluate_condition,
};
pub use judge::{ChainVerdict, Decision, FallbackDenial, Judge, Outcome, compile_bundle, judge};
pub use schema::{RequestPath, TicketPath};
pub use ticket::{
    Alignment, Baseline, CategoryPosture, OverallPosture, Posture, PostureLevel, Ticket,
    TicketClaims, TicketState, TicketVerifier, parse_base64_ticket, parse_jwt_ticket,
};
