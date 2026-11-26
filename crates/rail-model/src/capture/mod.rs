//! The capture plan: which calls the proxy records, and what of them; and the windows those calls
//! are counted in, one open at a time, each closed into a call record document.

mod call;
mod compile;
mod document;
mod plan;
mod window;
mod windowing;

pub use call::{
    CapturedCall, KeptValue, KeyReading, MeasureReading, PathReading, RecordReading, Replacement,
    Taken, Untaken,
};
#[cfg(feature = "conformance")]
pub(crate) use call::{answer, capture_environment, read_record};
pub use compile::{CompiledEntry, CompiledPlan, EntryRefusal, compile_plan};

pub use document::Document;
#[cfg(feature = "conformance")]
pub(crate) use plan::read_record_path;
pub use plan::{
    CatchAll, Combine, ContextKey, Entry, InvalidPlan, KeyFunction, Measure, MeasureFunction,
    OnMaxGroups, Plan, PlanBudget, Record, parse_plan,
};
pub use window::{BodyUnreadReason, ClosedWindow, Window};
pub use windowing::{CallOutcome, Recorded, Windowing};
