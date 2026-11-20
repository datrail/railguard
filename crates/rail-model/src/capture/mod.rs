//! The capture plan: which calls the proxy records, and what of them.

mod plan;

pub use plan::{
    Agent, Budget, CatchAllKey, Combine, ContentHash, ContextKey, Entry, Function, Measure,
    OnMaxGroups, Plan, PlanReading, PlanState, Refused, parse_plan,
};
