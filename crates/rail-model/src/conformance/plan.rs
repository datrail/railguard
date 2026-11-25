//! A plan written around one entry, for the layers reading an entry's match or a record.

use crate::capture::{CompiledPlan, compile_plan, parse_plan};

const MATCH: &str = r#"[{"path": ["request", "host"], "op": "existing"}]"#;
const ENTRY_RECORD: &str = r#"{"context_keys": [], "measures": [{"name": "call_count"}, {"name": "error_count"}, {"name": "request_bytes", "type": "sum", "function": "size", "paths": [["request", "body"]]}]}"#;
const CATCH_ALL_RECORD: &str = r#"{"context_keys": [{"name": "host"}], "measures": [{"name": "call_count"}, {"name": "error_count"}, {"name": "request_bytes", "type": "sum", "function": "size", "paths": [["request", "body"]]}]}"#;

/// The parts a case writes; the rest is fixed.
#[derive(Default)]
pub(super) struct OneEntry<'a> {
    pub(super) entry_match: Option<&'a str>,
    pub(super) entry_record: Option<&'a str>,
    pub(super) catch_all_record: Option<&'a str>,
}

impl OneEntry<'_> {
    pub(super) fn compile(&self) -> Result<CompiledPlan, String> {
        let text = format!(
            r#"{{"schema_version": "1.0", "content_hash": "sha256:0", "generated_at": "2026-11-10T09:58:02Z",
            "agent": {{"host_id": "host-1", "sandbox_name": "agent-1"}}, "plan_version": 1,
            "redaction_version": 1, "window_seconds": 900,
            "budget": {{"max_groups_per_window": 1000, "max_steps_per_window": 10000,
                "on_max_groups": "coarsen_to_catch_all", "max_unsent_windows": 8}},
            "entries": [{{"id": "entry", "match": {}, "record": {}}}],
            "catch_all": {{"id": "catch_all", "record": {}}}}}"#,
            self.entry_match.unwrap_or(MATCH),
            self.entry_record.unwrap_or(ENTRY_RECORD),
            self.catch_all_record.unwrap_or(CATCH_ALL_RECORD),
        );
        let plan = parse_plan(&text).map_err(|e| format!("the plan written: {e}"))?;
        Ok(compile_plan(plan))
    }
}
