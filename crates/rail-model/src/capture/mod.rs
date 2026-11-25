//! The capture plan: which calls the proxy records, and what of them; and the windows those calls
//! are counted in, one after another, each closed into a call record document.

mod call;
mod compile;
mod document;
mod plan;
mod window;
mod windowing;

pub use call::{
    CapturedCall, KeptValue, KeyReading, MeasureReading, ObservedCall, PathReading, RecordReading,
    Replacement, Taken, Untaken,
};
pub use compile::{CompiledEntry, CompiledPlan, EntryRefusal, compile_plan};

pub use document::Document;
pub use plan::{
    CatchAll, Combine, ContextKey, Entry, Function, InvalidPlan, Measure, OnMaxGroups, Plan,
    PlanBudget, Record, parse_plan,
};
pub use window::{BodyUnreadReason, ClosedWindow, Window};
pub use windowing::{Counted, Windowing};
