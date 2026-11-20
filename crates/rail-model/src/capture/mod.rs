//! The capture plan: which calls the proxy records, and what of them.

mod plan;

pub use plan::{
    CatchAll, CatchAllKey, CatchAllRecord, Combine, ContextKey, Entry, Function, InvalidPlan,
    Measure, OnMaxGroups, Plan, PlanBudget, Record, parse_plan,
};
