//! The capture plan: which calls the proxy records, and what of them.

mod call;
mod compile;
mod plan;

pub use call::{
    CapturedCall, KeptValue, KeyReading, MeasureReading, ObservedCall, PathReading, RecordReading,
    Replacement, Taken, Untaken,
};
pub use compile::{CompiledEntry, CompiledPlan, EntryRefusal, compile_plan};

pub use plan::{
    CatchAll, Combine, ContextKey, Entry, Function, InvalidPlan, Measure, OnMaxGroups, Plan,
    PlanBudget, Record, parse_plan,
};
