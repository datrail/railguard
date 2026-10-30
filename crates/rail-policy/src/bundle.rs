use std::collections::HashSet;
use std::fmt;

use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

/// The body of Rail Center's `GET /v1/policy-bundle`.
#[derive(Debug, Deserialize)]
pub struct Bundle {
    pub schema_version: SchemaVersion,
    pub content_hash: String,
    // `generated_at` goes here: nothing reads it yet, and it needs a date type.
    pub enforcement: Enforcement,
    /// The enabled policies, in evaluation order.
    pub policies: Vec<Policy>,
    pub bindings: Vec<Binding>,
    pub binding_fallback: Fallback,
    pub ticket_fallback: Fallback,
}

/// Rail Center writes `MAJOR.MINOR`, which reads as revision 0.
#[derive(Debug, PartialEq, Deserialize)]
#[serde(try_from = "String")]
pub struct SchemaVersion {
    pub major: u32,
    pub minor: u32,
    pub revision: u32,
}

impl TryFrom<String> for SchemaVersion {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        let numbers: Result<Vec<u32>, _> = text.split('.').map(str::parse).collect();
        let (major, minor, revision) = match numbers.as_deref() {
            Ok(&[major, minor]) => (major, minor, 0),
            Ok(&[major, minor, revision]) => (major, minor, revision),
            _ => return Err(format!("schema_version {text:?}")),
        };
        Ok(Self {
            major,
            minor,
            revision,
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct Enforcement {
    pub mode: EnforcementMode,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnforcementMode {
    None,
    Observe,
    Enforce,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Fallback {
    Pass,
    Block,
}

#[derive(Debug, Deserialize)]
pub struct Policy {
    pub id: Uuid,
    pub name: String,
    pub priority: i64,
    /// Carried unread: a bad condition only refuses a request that reaches it.
    pub condition: Value,
    pub action: Action,
    enabled: bool,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Block,
    Alert,
}

#[derive(Debug, Deserialize)]
pub struct Binding {
    pub endpoint_key: String,
    pub mode: BindingMode,
    pub policy_ids: Vec<Uuid>,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BindingMode {
    Gated,
    Open,
}

/// Why a bundle can't be used. It is refused whole.
#[derive(Debug)]
pub struct UnusableBundle(String);

impl fmt::Display for UnusableBundle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unusable policy bundle: {}", self.0)
    }
}

impl std::error::Error for UnusableBundle {}

pub fn parse_bundle(json: &str) -> Result<Bundle, UnusableBundle> {
    let mut bundle: Bundle =
        serde_json::from_str(json).map_err(|e| UnusableBundle(e.to_string()))?;

    bundle.policies.retain(|policy| policy.enabled);
    check_policies(&bundle.policies)?;
    check_bindings(&bundle.bindings)?;
    bundle
        .policies
        .sort_by_key(|policy| (policy.priority, policy.id));
    Ok(bundle)
}

fn refuse(reason: String) -> Result<(), UnusableBundle> {
    Err(UnusableBundle(reason))
}

fn check_policies(policies: &[Policy]) -> Result<(), UnusableBundle> {
    let mut seen = HashSet::new();
    for policy in policies {
        if !seen.insert(policy.id) {
            return refuse(format!("policy {} appears twice", policy.id));
        }
    }
    Ok(())
}

fn check_bindings(bindings: &[Binding]) -> Result<(), UnusableBundle> {
    let mut seen = HashSet::new();
    for binding in bindings {
        let key = &binding.endpoint_key;
        // The gateway matches keys without the slug, so these reach one endpoint.
        let local_key = key.split_once('#').map_or(key.as_str(), |(_, rest)| rest);
        if !seen.insert(local_key) {
            return refuse(format!("two bindings reach {local_key:?}"));
        }
        match (&binding.mode, binding.policy_ids.is_empty()) {
            (BindingMode::Open, false) => {
                return refuse(format!("open binding {key:?} names policies"));
            }
            // Read literally, this would disarm the endpoint.
            (BindingMode::Gated, true) => {
                return refuse(format!("gated binding {key:?} names no policy"));
            }
            _ => {}
        }
    }
    Ok(())
}
