use std::collections::HashSet;
use std::fmt;

use serde::Deserialize;
use uuid::Uuid;

use crate::condition::Condition;
use crate::endpoint::ResolvedKey;

/// The body of Rail Center's `GET /v1/policy-bundle`.
#[derive(Debug, Deserialize)]
pub struct Bundle {
    pub schema_version: SchemaVersion,
    /// Changes with every field but itself and `generated_at`. It orders nothing.
    pub content_hash: String,
    // `generated_at` goes here: nothing reads it yet, and it needs a date type.
    pub enforcement: Enforcement,
    /// In evaluation order, including the disabled ones Rail Center publishes.
    pub policies: Vec<Policy>,
    pub bindings: Vec<Binding>,
    /// For an endpoint with no binding: `block` denies, `pass` walks the whole chain.
    pub binding_fallback: Fallback,
    /// For a call with no usable ticket, asked first: `block` denies, `pass` goes on without it.
    pub ticket_fallback: Fallback,
}

/// `MAJOR.MINOR`, digits only.
#[derive(Debug, PartialEq, Deserialize)]
#[serde(try_from = "String")]
pub struct SchemaVersion {
    pub major: u32,
    pub minor: u32,
}

impl TryFrom<String> for SchemaVersion {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        // Digits only, since `u32::from_str` also takes a leading `+`.
        let read = |part: &str| -> Option<u32> {
            if !part.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            part.parse().ok()
        };
        let version = text.split_once('.').and_then(|(major, minor)| {
            Some(Self {
                major: read(major)?,
                minor: read(minor)?,
            })
        });
        version.ok_or_else(|| format!("schema_version {text:?}"))
    }
}

#[derive(Debug, Deserialize)]
pub struct Enforcement {
    pub mode: EnforcementMode,
}

/// How much of a verdict is acted on: `observe` judges and reports but refuses nothing.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnforcementMode {
    None,
    Observe,
    Enforce,
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Fallback {
    Pass,
    Block,
}

#[derive(Debug, Deserialize)]
pub struct Policy {
    pub id: Uuid,
    pub name: String,
    /// Lowest first, ties broken by id.
    pub priority: i64,
    pub condition: Condition,
    pub action: Action,
    pub enabled: bool,
}

/// `alert` records a match and never denies.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Block,
    Alert,
}

#[derive(Debug, Deserialize)]
pub struct Binding {
    pub endpoint_key: EndpointKey,
    pub mode: BindingMode,
    pub policy_ids: Vec<Uuid>,
}

/// `<slug>#<path>#<method>#<call>` on the wire: a slug, then the key a host resolves.
#[derive(Debug, PartialEq, Deserialize)]
#[serde(try_from = "String")]
pub struct EndpointKey {
    pub slug: String,
    pub resolved: ResolvedKey,
}

impl TryFrom<String> for EndpointKey {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        let Some((slug, rest)) = text.split_once('#').filter(|(slug, _)| !slug.is_empty()) else {
            return Err(format!("endpoint_key {text:?}"));
        };
        let resolved = rest.parse().map_err(|_| format!("endpoint_key {text:?}"))?;
        Ok(Self {
            slug: slug.to_string(),
            resolved,
        })
    }
}

/// `gated` applies only `policy_ids`; `open` applies no policy.
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

    check_policies(&bundle.policies)?;
    check_bindings(&bundle.bindings, &bundle.policies)?;
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

fn check_bindings(bindings: &[Binding], policies: &[Policy]) -> Result<(), UnusableBundle> {
    let known_policies: HashSet<Uuid> = policies.iter().map(|policy| policy.id).collect();
    let mut seen = HashSet::new();
    for binding in bindings {
        let key = &binding.endpoint_key;
        // The gateway matches keys without the slug, so these reach one endpoint.
        if !seen.insert(&key.resolved) {
            return refuse(format!("two bindings reach {key:?}"));
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
        // Stricter than the contract, where an unknown id binds nothing: Rail Center never sends one.
        if let Some(id) = binding
            .policy_ids
            .iter()
            .find(|id| !known_policies.contains(id))
        {
            return refuse(format!("binding {key:?} names unknown policy {id}"));
        }
    }
    Ok(())
}
