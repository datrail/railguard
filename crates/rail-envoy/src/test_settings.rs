//! Settings read as `main` reads them, for the unit tests.

use rail_runtime::{ProtectedHosts, Refused, Settings, Startup, Variables, read_protected_hosts};

/// What `reader` reads of `pairs`, and what that logs, or every refusal.
pub fn read<T>(
    pairs: &[(&str, &str)],
    reader: impl FnOnce(&mut Settings) -> Result<T, Refused>,
) -> Result<(T, Startup), Vec<String>> {
    let variables: Variables = pairs.iter().copied().collect();
    let mut settings = Settings::new(&variables);
    let read = reader(&mut settings);
    settings.finish("RAIL_", read).map_err(|errors| errors.0)
}

/// The protected hosts `value` lists, which must be accepted.
pub fn protect(value: &str) -> ProtectedHosts {
    let pairs = [("RAIL_PROXY_PROTECTED_HOSTS", value)];
    read(&pairs, |settings| read_protected_hosts(settings, true))
        .unwrap()
        .0
}
