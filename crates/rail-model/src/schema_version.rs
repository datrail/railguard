//! The version of a document's shape, as Rail Center writes it.

use serde::Deserialize;

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
