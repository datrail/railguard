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

/// A refusal, already recorded in the [`Settings`] that read it; only `Settings` makes one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Refused(());

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
    // Read for their text, not only checked with `is_set`.
    texts: BTreeSet<&'static str>,
    secrets: BTreeSet<&'static str>,
    entries: Vec<Entry>,
}

impl<'a> Settings<'a> {
    pub fn new(variables: &'a Variables) -> Self {
        Self {
            variables,
            errors: Vec::new(),
            read: Vec::new(),
            texts: BTreeSet::new(),
            secrets: BTreeSet::new(),
            entries: Vec::new(),
        }
    }

    /// The setting `name`, to read or refuse.
    pub fn get<'s>(&'s mut self, name: &'static str) -> Setting<'s, 'a> {
        Setting {
            settings: self,
            name,
            required: false,
            because: None,
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

    /// `name`, unset, as taken from the platform it runs on.
    pub fn describe_detected(&mut self, name: &'static str, value: impl fmt::Display) {
        self.mark_read(name);
        let entry = self.build_entry(name, value.to_string(), Level::Info);
        self.entries.push(Entry {
            source: Source::Detected,
            ..entry
        });
    }

    /// The environment read, for what isn't a setting, such as the platform's own variables;
    /// reading it records nothing.
    pub fn variables(&self) -> &'a Variables {
        self.variables
    }

    /// A secret, only ever as set or unset.
    pub fn describe_secret(&mut self, name: &'static str, secret: Option<&Secret>) {
        self.describe(name, if secret.is_some() { "set" } else { "unset" });
    }

    /// What `read` reads and what to log of the settings, as `finish` gives them; on any
    /// refusal, they're logged and the process exits as a refused configuration does.
    pub fn finish_or_exit<T>(
        mut self,
        prefix: &str,
        read: impl FnOnce(&mut Self) -> Result<T, Refused>,
    ) -> (T, Startup) {
        let value = read(&mut self);
        self.finish(prefix, value)
            .unwrap_or_else(|errors| errors.exit())
    }

    /// `value` and what to log of the settings, or every refusal.
    ///
    /// Described entries are in the order their names were first read. A name read, set, but not
    /// described follows them alphabetically, as read: its value, or only `set` for a secret or a
    /// name only checked with `is_set`. `prefix` names the variables that are likely typos when
    /// nothing reads them.
    pub fn finish<T>(
        self,
        prefix: &str,
        value: Result<T, Refused>,
    ) -> Result<(T, Startup), ConfigErrors> {
        if !self.errors.is_empty() {
            return Err(ConfigErrors(self.errors));
        }
        let value = value.expect("no errors recorded means Ok");
        let unread = self.find_unread(prefix);
        let mut entries = Vec::new();
        let mut rest = BTreeSet::new();
        for &name in &self.read {
            let before = entries.len();
            entries.extend(
                self.entries
                    .iter()
                    .filter(|entry| entry.name == name)
                    .cloned(),
            );
            if entries.len() == before && self.variables.is_set(name) {
                rest.insert(name);
            }
        }
        for name in rest {
            entries.push(self.build_entry(name, self.format_as_read(name), Level::Info));
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

    // Only for a name that's set.
    fn format_as_read(&self, name: &str) -> String {
        if self.secrets.contains(name) || !self.texts.contains(name) {
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
        Refused(())
    }
}

/// One setting: what it must be, then one read (`text`, `parsed`, `boolean` or `secret`), which
/// is `Ok(None)` when the value is unset or blank, and records any refusal.
///
/// `is_set`, `retired` and `refuse` skip a read's checks: they look at the value, or refuse it, as
/// is. A read, `is_set`, `describe` or `warn` records the name as read, and `secret` as a secret
/// too.
#[must_use = "a setting checks nothing until it's read"]
pub struct Setting<'s, 'a> {
    settings: &'s mut Settings<'a>,
    name: &'static str,
    required: bool,
    because: Option<&'s str>,
    max_chars: Option<usize>,
}

impl<'s, 'a> Setting<'s, 'a> {
    /// Refused when unset, if `condition`.
    pub fn required_if(mut self, condition: bool) -> Self {
        self.required = condition;
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
    pub fn is_set(&mut self) -> bool {
        self.settings.mark_read(self.name);
        self.settings.variables.is_set(self.name)
    }

    /// Refuses it if it's set: nothing reads it any more, and `instead` says what does.
    pub fn retired(mut self, instead: &str) -> Result<(), Refused> {
        if self.settings.variables.is_set(self.name) {
            return Err(self.reject(format_args!("is no longer read; {instead}")));
        }
        Ok(())
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
        self.settings.texts.insert(self.name);
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
        match (self.required, self.because) {
            (false, _) => Ok(None),
            (true, Some(reason)) => Err(self.reject(format_args!(
                "is required because {reason}, and is unset or blank"
            ))),
            (true, None) => Err(self.reject("is required and is unset or blank")),
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
    /// Writes the refusals to stderr, as one ERROR line each in a JSON log format, and exits with
    /// [`EXIT_CODE`].
    pub fn exit(&self) -> ! {
        crate::logging::log_refusals(self);
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

/// Where an entry's value comes from: its variable, a default when that's unset, or what the
/// platform it runs on says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Env,
    Default,
    Detected,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Env => "env",
            Self::Default => "default",
            Self::Detected => "detected",
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
    /// Each setting described, with the value it takes after parsing and defaults, then each one
    /// set that nothing described, alphabetically and as read: its value, or `set` for a secret or
    /// a name only checked with `is_set`.
    pub entries: Vec<Entry>,
    /// Each variable under the prefix that nothing read, likely a typo.
    pub unread: Vec<String>,
}
