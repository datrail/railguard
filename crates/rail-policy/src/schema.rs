//! The paths a condition may name, by source.

/// A path under `ticket`, one per typed claim.
#[derive(Debug, PartialEq)]
pub(crate) enum TicketPath {
    Whole,
    Id,
    AgentId,
    SandboxId,
    SandboxType,
    EnvironmentFingerprint,
    Skills,
    Posture,
    Overall,
    OverallScore,
    OverallLevel,
    Categories,
    Category(String),
    CategoryScore(String),
    CategoryCapped(String),
    Alignment,
    Drifted,
    Baseline,
    BaselineId,
    BaselineLabel,
}

impl TicketPath {
    /// A claim this reader doesn't know would read as absent, so a rule on it wouldn't fire.
    pub(crate) fn parse(keys: &[&str]) -> Option<Self> {
        Some(match keys {
            [] => Self::Whole,
            ["id"] => Self::Id,
            ["agent_id"] => Self::AgentId,
            ["sandbox_id"] => Self::SandboxId,
            ["sandbox_type"] => Self::SandboxType,
            ["environment_fingerprint"] => Self::EnvironmentFingerprint,
            ["skills"] => Self::Skills,
            ["posture"] => Self::Posture,
            ["posture", "overall"] => Self::Overall,
            ["posture", "overall", "score"] => Self::OverallScore,
            ["posture", "overall", "level"] => Self::OverallLevel,
            ["posture", "categories"] => Self::Categories,
            ["posture", "categories", name] => Self::Category(name.to_string()),
            ["posture", "categories", name, "score"] => Self::CategoryScore(name.to_string()),
            ["posture", "categories", name, "capped"] => Self::CategoryCapped(name.to_string()),
            ["alignment"] => Self::Alignment,
            ["alignment", "drifted"] => Self::Drifted,
            ["alignment", "baseline"] => Self::Baseline,
            ["alignment", "baseline", "id"] => Self::BaselineId,
            ["alignment", "baseline", "label"] => Self::BaselineLabel,
            _ => return None,
        })
    }
}

/// A path under `request`.
#[derive(Debug, PartialEq)]
pub(crate) enum RequestPath {
    Whole,
    EndpointKey,
    Headers,
    /// Matched exactly, so a name with an upper-case letter never exists.
    Header(String),
}

impl RequestPath {
    /// `None` outside what Rail Center publishes: `endpoint_key` and `headers.<name>`.
    pub(crate) fn parse(keys: &[&str]) -> Option<Self> {
        Some(match keys {
            [] => Self::Whole,
            ["endpoint_key"] => Self::EndpointKey,
            ["headers"] => Self::Headers,
            ["headers", name] => Self::Header(name.to_string()),
            _ => return None,
        })
    }
}
