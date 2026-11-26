//! `capture/plans.json`: named plans, which a case names, its own entries in place of the plan's
//! where it gives them; or a case's plan written whole.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

use crate::capture::{CompiledPlan, compile_plan, parse_plan};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Plans {
    plans: BTreeMap<String, Value>,
}

pub(super) struct NamedPlans(BTreeMap<String, Value>);

/// A case's plan: a name, or the plan written whole.
#[derive(Deserialize)]
#[serde(untagged)]
pub(super) enum PlanSpec {
    Named(String),
    Whole(Value),
}

impl NamedPlans {
    pub(super) fn read(text: &str) -> Result<Self, String> {
        let file: Plans =
            serde_json::from_str(text).map_err(|e| format!("capture/plans.json: {e}"))?;
        Ok(Self(file.plans))
    }

    fn get(&self, name: &str) -> Result<Value, String> {
        self.0
            .get(name)
            .cloned()
            .ok_or_else(|| format!("no plan {name:?}"))
    }

    /// `spec`'s plan, `entries` in place of a named plan's, compiled.
    pub(super) fn compile(
        &self,
        spec: &PlanSpec,
        entries: Option<&Value>,
    ) -> Result<CompiledPlan, String> {
        let mut plan = match spec {
            PlanSpec::Named(name) => self.get(name)?,
            PlanSpec::Whole(_) if entries.is_some() => {
                return Err("entries beside a plan written whole".into());
            }
            PlanSpec::Whole(plan) => plan.clone(),
        };
        if let Some(entries) = entries {
            plan["entries"] = entries.clone();
        }
        compile_written(&plan)
    }

    /// The plan `name`, `edit` applied to it, compiled.
    pub(super) fn compile_edited(
        &self,
        name: &str,
        edit: impl FnOnce(&mut Value) -> Result<(), String>,
    ) -> Result<CompiledPlan, String> {
        let mut plan = self.get(name)?;
        edit(&mut plan)?;
        compile_written(&plan)
    }
}

fn compile_written(plan: &Value) -> Result<CompiledPlan, String> {
    let plan = parse_plan(&plan.to_string()).map_err(|e| format!("the plan: {e}"))?;
    Ok(compile_plan(plan))
}
