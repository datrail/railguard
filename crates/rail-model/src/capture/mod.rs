//! The capture plan: which calls the proxy records, and what of them.

mod plan;

pub use plan::{
    Budget, CatchAll, CatchAllKey, CatchAllRecord, Combine, ContextKey, Entry, Function,
    InvalidPlan, Measure, OnMaxGroups, Plan, Record, parse_plan,
};
