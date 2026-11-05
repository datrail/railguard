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
    refusals: usize,
}

impl<'a> Settings<'a> {
    pub fn new(variables: &'a Variables) -> Self {
        Self {
            variables,
            errors: Vec::new(),
            refusals: 0,
        }
    }

    /// The setting `name`, to read or refuse.
    pub fn get<'s>(&'s mut self, name: &'s str) -> Setting<'s, 'a> {
        Setting {
            settings: self,
            name,
            required: false,
            because: None,
            max_chars: None,
        }
    }

    /// Refusals so far, repeats included, so a caller can tell whether its own reads refused.
    pub fn refusals(&self) -> usize {
        self.refusals
    }

    /// Every refusal recorded, if any.
    pub fn finish(self) -> Result<(), ConfigErrors> {
        if self.errors.is_empty() {
            Ok(())
        } else {
            Err(ConfigErrors(self.errors))
        }
    }

    // Listed once however often the same value is read, and counted each time.
    fn record(&mut self, refusal: String) {
        self.refusals += 1;
        if !self.errors.contains(&refusal) {
            self.errors.push(refusal);
        }
    }
}

/// One setting: what it must be, then one read (`text`, `parsed`, `boolean` or `secret`), which
/// records any refusal and is `None` when the value is unset, blank or refused.
///
/// `is_set`, `retired` and `refuse` aren't reads: they look at the value, or refuse it, as is.
#[must_use = "a setting checks nothing until it's read"]
pub struct Setting<'s, 'a> {
    settings: &'s mut Settings<'a>,
    name: &'s str,
    required: bool,
    because: Option<&'s str>,
    max_chars: Option<usize>,
}

impl<'s, 'a> Setting<'s, 'a> {
    /// Refused when unset, if `required`.
    pub fn required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }

    /// Why it's required, said when it's refused for being unset.
    pub fn because(mut self, reason: &'s str) -> Self {
        self.because = Some(reason);
        self
    }

    /// Refused past `max` characters.
    pub fn max_chars(mut self, max: usize) -> Self {
        self.max_chars = Some(max);
        self
    }

    /// Whether it holds anything but whitespace.
    pub fn is_set(&self) -> bool {
        self.raw()
            .is_some_and(|value| value.to_str().is_none_or(|text| !text.trim().is_empty()))
    }

    /// Refuses it if it's set: nothing reads it any more, and `instead` says what does.
    pub fn retired(mut self, instead: &str) {
        if self.is_set() {
            self.reject(format_args!("is no longer read; {instead}"));
        }
    }

    /// Refuses it, `reason` following its name.
    /// The reason is logged, so it quotes a value only through `sanitize`, and never a secret's.
    pub fn refuse(mut self, reason: impl fmt::Display) {
        self.reject(reason);
    }

    /// The trimmed text.
    pub fn text(mut self) -> Option<&'a str> {
        self.read()
    }

    /// The text read by `parse`, whose refusal follows the name.
    /// The reason is logged, so it quotes a value only through `sanitize`, and never a secret's.
    pub fn parsed<T>(mut self, parse: impl FnOnce(&str) -> Result<T, String>) -> Option<T> {
        let text = self.read()?;
        parse(text).map_err(|reason| self.reject(reason)).ok()
    }

    /// `true` or `false`, case folded.
    pub fn boolean(self) -> Option<bool> {
        self.parsed(|text| match text.to_ascii_lowercase().as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(format!("must be true or false, got: {}", sanitize(text))),
        })
    }

    /// The text as a secret, which no refusal quotes.
    pub fn secret(mut self) -> Option<Secret> {
        self.read().map(|text| Secret(text.to_owned()))
    }

    // The checks every read makes, in this order: unset, UTF-8, blank, length.
    fn read(&mut self) -> Option<&'a str> {
        let Some(value) = self.raw() else {
            return self.missing();
        };
        let Some(text) = value.to_str() else {
            self.reject("is not valid UTF-8");
            return None;
        };
        let text = text.trim();
        if text.is_empty() {
            return self.missing();
        }
        if let Some(max) = self.max_chars {
            let chars = text.chars().count();
            if chars > max {
                self.reject(format_args!(
                    "has {chars} characters, past the {max} allowed"
                ));
                return None;
            }
        }
        Some(text)
    }

    fn missing(&mut self) -> Option<&'a str> {
        match (self.required, self.because) {
            (true, Some(reason)) => {
                self.reject(format_args!(
                    "is required because {reason}, and is unset or blank"
                ));
            }
            (true, None) => self.reject("is required and is unset or blank"),
            (false, _) => {}
        }
        None
    }

    fn reject(&mut self, reason: impl fmt::Display) {
        let refusal = format!("{} {reason}", self.name);
        self.settings.record(refusal);
    }

    fn raw(&self) -> Option<&'a OsStr> {
        self.settings
            .variables
            .0
            .get(OsStr::new(self.name))
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
        f.write_str("Invalid configuration detected:")?;
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
    pub fn new(value: String) -> Self {
        Self(value)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}
