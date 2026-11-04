use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fmt;

use rail_policy::sanitize;

/// The exit status of a process whose configuration is refused.
pub const EXIT_CODE: i32 = 2;

/// A snapshot of the environment, so a test passes its own instead of the process's.
#[derive(Clone, Debug, Default)]
pub struct Variables(BTreeMap<OsString, OsString>);

impl Variables {
    pub fn from_process() -> Self {
        Self(std::env::vars_os().collect())
    }
}

impl<K: Into<OsString>, V: Into<OsString>> FromIterator<(K, V)> for Variables {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(pairs: I) -> Self {
        Self(
            pairs
                .into_iter()
                .map(|(name, value)| (name.into(), value.into()))
                .collect(),
        )
    }
}

/// Reads typed settings, collecting every refusal so an operator fixes them in one round.
///
/// Blank means unset. A value is trimmed before it's read, since a value written to a file keeps
/// its trailing newline.
pub struct Settings<'a> {
    variables: &'a Variables,
    errors: Vec<String>,
}

impl<'a> Settings<'a> {
    pub fn new(variables: &'a Variables) -> Self {
        Self {
            variables,
            errors: Vec::new(),
        }
    }

    /// Whether `name` holds anything but whitespace.
    pub fn is_set(&self, name: &str) -> bool {
        self.raw(name)
            .is_some_and(|value| value.to_str().is_none_or(|text| !text.trim().is_empty()))
    }

    /// The trimmed value of `name`, or `None` when it's unset, blank or refused.
    pub fn text(&mut self, name: &str) -> Option<&'a str> {
        let value = self.raw(name)?;
        let Some(text) = value.to_str() else {
            self.refuse(format!("{name} is not valid UTF-8"));
            return None;
        };
        Some(text.trim()).filter(|text| !text.is_empty())
    }

    /// As [`Settings::text`], refusing an unset or blank value.
    pub fn required(&mut self, name: &str) -> Option<&'a str> {
        let text = self.text(name);
        if text.is_none() && !self.is_set(name) {
            self.refuse(format!("{name} is required and is unset or blank"));
        }
        text
    }

    /// The value of `name` read by `parse`, whose refusal follows the variable's name.
    pub fn parse<T>(
        &mut self,
        name: &str,
        parse: impl FnOnce(&str) -> Result<T, String>,
    ) -> Option<T> {
        let text = self.text(name)?;
        parse(text)
            .map_err(|reason| self.refuse(format!("{name} {reason}")))
            .ok()
    }

    /// `true` or `false`, case folded, or `default` when unset.
    pub fn boolean(&mut self, name: &str, default: bool) -> bool {
        self.parse(name, |text| match text.to_ascii_lowercase().as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(format!("must be true or false, got: {}", sanitize(text))),
        })
        .unwrap_or(default)
    }

    /// The value of `name` as a secret, which no refusal quotes.
    pub fn secret(&mut self, name: &str) -> Option<Secret> {
        self.text(name).map(|text| Secret(text.to_owned()))
    }

    /// Refuses `name` if it's set: nothing reads it any more, and `instead` says what does.
    pub fn retired(&mut self, name: &str, instead: &str) {
        if self.is_set(name) {
            self.refuse(format!("{name} is no longer read; {instead}"));
        }
    }

    /// How many refusals are recorded so far.
    pub fn refusals(&self) -> usize {
        self.errors.len()
    }

    pub fn refuse(&mut self, message: impl Into<String>) {
        self.errors.push(message.into());
    }

    /// Every refusal recorded, if any.
    pub fn finish(self) -> Result<(), ConfigErrors> {
        if self.errors.is_empty() {
            Ok(())
        } else {
            Err(ConfigErrors(self.errors))
        }
    }

    fn raw(&self, name: &str) -> Option<&'a OsStr> {
        self.variables
            .0
            .get(OsStr::new(name))
            .map(OsString::as_os_str)
    }
}

/// Every reason a configuration was refused, one per line.
#[derive(Debug, PartialEq, Eq)]
pub struct ConfigErrors(pub Vec<String>);

impl ConfigErrors {
    /// Writes the refusals to stderr and exits with [`EXIT_CODE`].
    pub fn exit(&self) -> ! {
        eprintln!("{self}");
        std::process::exit(EXIT_CODE)
    }
}

impl fmt::Display for ConfigErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the configuration is refused:")?;
        for error in &self.0 {
            write!(f, "\n  {error}")?;
        }
        Ok(())
    }
}

/// A value that never displays: no `Display`, and `Debug` hides it.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}
