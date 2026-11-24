//! What a holder reads of a Rail Center document before reading it whole, so a document of a
//! shape it doesn't read is still told by its hash and version.

use rail_model::schema_version::SchemaVersion;
use serde::de::DeserializeOwned;
use serde_json::Value;
use serde_json::error::Category;

/// `body` as text, and its peek `T`, or why it isn't of `T`'s shape (not an object, a field it
/// reads repeated); why neither, if `body` isn't UTF-8 or JSON.
pub(crate) fn peek<T: DeserializeOwned>(body: &[u8]) -> Result<(&str, Result<T, String>), String> {
    let Ok(json) = std::str::from_utf8(body) else {
        return Err("the response isn't UTF-8".to_owned());
    };
    match serde_json::from_str::<T>(json) {
        Ok(peek) => Ok((json, Ok(peek))),
        Err(error) if error.classify() == Category::Data => Ok((json, Err(locate(&error)))),
        Err(error) => Err(format!("the response isn't JSON ({})", locate(&error))),
    }
}

// Where, not what: serde's message can quote a value.
fn locate(error: &serde_json::Error) -> String {
    format!(
        "{:?} error at line {}, column {}",
        error.classify(),
        error.line(),
        error.column()
    )
}

/// A peeked field, where it's a string.
pub(crate) fn text(field: Option<&Value>) -> Option<&str> {
    field.and_then(Value::as_str)
}

/// A peeked `schema_version` as written, and read, where it's `MAJOR.MINOR`.
pub(crate) fn schema_version(field: Option<&Value>) -> Option<(&str, SchemaVersion)> {
    let declared = text(field)?;
    let version = SchemaVersion::try_from(declared.to_owned()).ok()?;
    Some((declared, version))
}
