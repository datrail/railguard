//! Conditions: their grammar, Rail Center's check of each, and their evaluation.

mod check;
mod environment;
mod evaluate;
mod grammar;

pub use check::{
    ConditionProblem, LocationSegment, OperandShape, ProblemCode, ProblemScope, TicketClaimsSchema,
    check_condition,
};
pub use environment::{Environment, Headers};
pub use evaluate::{
    Budget, CompiledCondition, DEFAULT_STEP_LIMIT, Evaluation, Lookup, Refusal, Verdict,
    compile_condition, evaluate_condition,
};
pub use grammar::{Condition, Path, Predicate, RequestPath, Scalar, TicketPath};
