use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fmt;

use rail_policy::sanitize;

/// The exit status of a process whose configuration is refused.
pub const EXIT_CODE: u8 = 2;

/// A snapshot of the environment, so a test passes its own instead of the process's.
#[derive(Clone, Debug, Default)]
pub struct Variables(BTreeMap<OsString, OsString>);

impl Variables {
    pub fn from_process() -> Self {
        Self(std::env::vars_os().collect())
    }

    /// Whether `name` holds anything but whitespace.
    pub fn is_set(&self, name: &str) -> bool {
        self.0.get(OsStr::new(name)).is_some_and(holds_text)
    }
}

// Blank is unset; a value that isn't UTF-8 is set, to be refused when it's read.
fn holds_text(value: &OsString) -> bool {
    value.to_str().is_none_or(|text| !text.trim().is_empty())
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

/// A refusal, already recorded in the [`Settings`] that read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Refused;

/// Reads typed settings, collecting every refusal so an operator fixes them in one round, and
/// each setting's entry for the startup log.
///
/// Blank means unset. A value is trimmed before it's read, since a value written to a file keeps
/// its trailing newline.
pub struct Settings<'a> {
    variables: &'a Variables,
    errors: Vec<String>,
    // In the order first read, which orders the entries.
    read: Vec<&'static str>,
    secrets: BTreeSet<&'static str>,
    entries: Vec<Entry>,
}

impl<'a> Settings<'a> {
    pub fn new(variables: &'a Variables) -> Self {
        Self {
            variables,
            errors: Vec::new(),
            read: Vec::new(),
            secrets: BTreeSet::new(),
            entries: Vec::new(),
        }
    }

    /// The setting `name`, to read or refuse.
    pub fn get<'s>(&'s mut self, name: &'static str) -> Setting<'s, 'a> {
        Setting {
            settings: self,
            name,
            required: None,
            max_chars: None,
        }
    }

    /// `name` as it's used, after parsing and defaults.
    pub fn describe(&mut self, name: &'static str, value: impl fmt::Display) {
        self.push(name, value, Level::Info);
    }

    /// `name` as it's used, a value that weakens what `weakens` says.
    pub fn warn(&mut self, name: &'static str, value: impl fmt::Display, weakens: &'static str) {
        self.push(name, value, Level::Warn(weakens));
    }

    /// A secret, only ever as set or unset.
    pub fn describe_secret(&mut self, name: &'static str, secret: Option<&Secret>) {
        self.describe(name, if secret.is_some() { "set" } else { "unset" });
    }

    /// `value` and what to log of the settings, or every refusal.
    ///
    /// A name read but not described gets an entry as read: its value, only `set` for a secret, or
    /// `unset`. Entries are in the order their names were first read. `prefix` names the
    /// variables that are likely typos when nothing reads them.
    pub fn finish<T>(self, prefix: &str, value: Option<T>) -> Result<(T, Startup), ConfigErrors> {
        if !self.errors.is_empty() {
            return Err(ConfigErrors(self.errors));
        }
        let value = value.expect("a reader returns its value when nothing is refused");
        let unread = self.find_unread(prefix);
        let mut entries = Vec::new();
        for &name in &self.read {
            let before = entries.len();
            entries.extend(
                self.entries
                    .iter()
                    .filter(|entry| entry.name == name)
                    .cloned(),
            );
            if entries.len() == before {
                entries.push(self.build_entry(name, self.format_as_read(name), Level::Info));
            }
        }
        Ok((value, Startup { entries, unread }))
    }

    // The variables set whose name starts with `prefix`, and which nothing read; a retired one set
    // is refused, so it never gets here.
    fn find_unread(&self, prefix: &str) -> Vec<String> {
        self.variables
            .0
            .iter()
            .filter(|(name, value)| {
                name.as_encoded_bytes().starts_with(prefix.as_bytes()) && holds_text(value)
            })
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .filter(|name| !self.read.contains(&name.as_str()))
            .collect()
    }

    fn format_as_read(&self, name: &str) -> String {
        if !self.variables.is_set(name) {
            "unset".to_owned()
        } else if self.secrets.contains(name) {
            "set".to_owned()
        } else {
            let value = self.variables.0[OsStr::new(name)].to_string_lossy();
            value.trim().to_owned()
        }
    }

    fn push(&mut self, name: &'static str, value: impl fmt::Display, level: Level) {
        self.mark_read(name);
        let entry = self.build_entry(name, value.to_string(), level);
        self.entries.push(entry);
    }

    fn build_entry(&self, name: &'static str, value: String, level: Level) -> Entry {
        let source = if self.variables.is_set(name) {
            Source::Env
        } else {
            Source::Default
        };
        Entry {
            name,
            value,
            level,
            source,
        }
    }

    fn mark_read(&mut self, name: &'static str) {
        if !self.read.contains(&name) {
            self.read.push(name);
        }
    }

    // Listed once however often the same value is read.
    fn record(&mut self, refusal: String) -> Refused {
        if !self.errors.contains(&refusal) {
            self.errors.push(refusal);
        }
        Refused
    }
}

/// One setting: what it must be, then one read (`text`, `parsed`, `boolean` or `secret`), which
/// is `Ok(None)` when the value is unset or blank, and records any refusal.
///
/// `is_set`, `retired` and `refuse` skip a read's checks: they look at the value, or refuse it, as
/// is. A read or `is_set` records the name as read, and `secret` as a secret too.
#[must_use = "a setting checks nothing until it's read"]
pub struct Setting<'s, 'a> {
    settings: &'s mut Settings<'a>,
    name: &'static str,
    // Why it's required, if it is.
    required: Option<&'s str>,
    max_chars: Option<usize>,
}

impl<'s, 'a> Setting<'s, 'a> {
    /// Refused when unset, if `condition`; `because` says why it's required.
    pub fn required_if(mut self, condition: bool, because: &'s str) -> Self {
        self.required = condition.then_some(because);
        self
    }

    /// Refused past `max` characters.
    pub fn max_chars(mut self, max: usize) -> Self {
        self.max_chars = Some(max);
        self
    }

    /// Whether it holds anything but whitespace.
    pub fn is_set(&mut self) -> bool {
        self.settings.mark_read(self.name);
        self.settings.variables.is_set(self.name)
    }

    /// Refuses it if it's set: nothing reads it any more, and `instead` says what does.
    pub fn retired(mut self, instead: &str) {
        if self.settings.variables.is_set(self.name) {
            self.reject(format_args!("is no longer read; {instead}"));
        }
    }

    /// Refuses it, `reason` following its name.
    /// The reason is printed, so it quotes a value only through `sanitize`, and never a secret's.
    pub fn refuse(mut self, reason: impl fmt::Display) -> Refused {
        self.reject(reason)
    }

    /// The trimmed text.
    pub fn text(mut self) -> Result<Option<&'a str>, Refused> {
        self.read()
    }

    /// The text read by `parse`, whose refusal follows the name.
    /// The reason is printed, so it quotes a value only through `sanitize`, and never a secret's.
    pub fn parsed<T>(
        mut self,
        parse: impl FnOnce(&str) -> Result<T, String>,
    ) -> Result<Option<T>, Refused> {
        let Some(text) = self.read()? else {
            return Ok(None);
        };
        parse(text).map(Some).map_err(|reason| self.reject(reason))
    }

    /// `true` or `false`, case folded.
    pub fn boolean(self) -> Result<Option<bool>, Refused> {
        self.parsed(|text| match text.to_ascii_lowercase().as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(format!("must be true or false, got: {}", sanitize(text))),
        })
    }

    /// The text as a secret, which no refusal quotes.
    pub fn secret(mut self) -> Result<Option<Secret>, Refused> {
        self.settings.secrets.insert(self.name);
        Ok(self.read()?.map(|text| Secret(text.to_owned())))
    }

    // The checks every read makes, in this order: unset, UTF-8, blank, length.
    fn read(&mut self) -> Result<Option<&'a str>, Refused> {
        self.settings.mark_read(self.name);
        let Some(value) = self.raw() else {
            return self.missing();
        };
        let Some(text) = value.to_str() else {
            return Err(self.reject("is not valid UTF-8"));
        };
        let text = text.trim();
        if text.is_empty() {
            return self.missing();
        }
        if let Some(max) = self.max_chars {
            let chars = text.chars().count();
            if chars > max {
                return Err(self.reject(format_args!(
                    "has {chars} characters, past the {max} allowed"
                )));
            }
        }
        Ok(Some(text))
    }

    fn missing(&mut self) -> Result<Option<&'a str>, Refused> {
        match self.required {
            Some(reason) => Err(self.reject(format_args!(
                "is required because {reason}, and is unset or blank"
            ))),
            None => Ok(None),
        }
    }

    fn reject(&mut self, reason: impl fmt::Display) -> Refused {
        let refusal = format!("{} {reason}", self.name);
        self.settings.record(refusal)
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
        std::process::exit(EXIT_CODE.into())
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

/// How an entry is logged: a warning says what its value weakens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn(&'static str),
}

/// Where an entry's value comes from: its variable, or a default when that's unset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Env,
    Default,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Env => "env",
            Self::Default => "default",
        }
    }
}

/// One setting as it's used, or as read if nothing used it, its value as given: the log escapes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: &'static str,
    pub value: String,
    pub level: Level,
    pub source: Source,
}

/// What a read configuration logs at startup.
#[derive(Debug, PartialEq, Eq)]
pub struct Startup {
    /// Each setting read, with the value it takes after parsing and defaults.
    pub entries: Vec<Entry>,
    /// Each variable under the prefix that nothing read, likely a typo.
    pub unread: Vec<String>,
}
