use std::fmt;

use reqwest::Url;

use crate::config::{Refused, Secret, Settings};

pub(crate) const DISABLED: &str = "RAIL_DISABLED";
pub(crate) const CENTER_URL: &str = "RAIL_CENTER_URL";
pub(crate) const ALLOW_HTTP: &str = "RAIL_ALLOW_HTTP";
pub(crate) const AUTH_MODE: &str = "RAIL_AUTH_MODE";
pub(crate) const AUTH_TOKEN: &str = "RAIL_AUTH_TOKEN";

// Why Rail Center's settings and the ticket's are required, when they are.
pub(crate) const USED: &str = "RAIL_DISABLED isn't true";

// What Rail Center mints for a component: the prefix, then `secrets.token_urlsafe(32)`.
const TOKEN_PREFIX: &str = "rc_svc_";
const TOKEN_SECRET_CHARS: usize = 43;

#[derive(Debug)]
pub struct RailCenterSettings {
    pub url: CenterUrl,
    /// Whether `RAIL_ALLOW_HTTP` allows an http:// URL.
    pub allow_http: bool,
    /// The `rc_svc_` token sent as a bearer credential, under `RAIL_AUTH_MODE=bearer`.
    pub token: Option<Secret>,
}

/// Whether `RAIL_DISABLED` is true.
pub fn read_disabled(settings: &mut Settings) -> Result<bool, Refused> {
    let disabled = settings.get(DISABLED).boolean()?.unwrap_or(false);
    if disabled {
        settings.warn(
            DISABLED,
            disabled,
            "Rail Center isn't used: nothing is fetched from it",
        );
    } else {
        settings.describe(DISABLED, disabled);
    }
    Ok(disabled)
}

/// The Rail Center settings, every one set judged, `None` if the URL or token is missing; both
/// are required if `required`, which is whether Rail Center is used. Refuses the retired names
/// too, which aren't described, since none can be set.
pub fn read_rail_center(
    settings: &mut Settings,
    required: bool,
) -> Result<Option<RailCenterSettings>, Refused> {
    // Here, not with `RAIL_DISABLED`: refusing one mustn't change what's required.
    let disable = format!("Rail Center is used unless {DISABLED}=true");
    let retired = [
        settings.get("RAIL_PLUGIN_ENABLED").retired(&disable),
        settings.get("RAIL_TICKET_MODE").retired(&disable),
        settings
            .get("RAIL_PROXY_ALLOW_INSECURE_CREDENTIAL")
            .retired(&format!("{ALLOW_HTTP}=true allows an http:// {CENTER_URL}")),
    ];
    let url = settings
        .get(CENTER_URL)
        .required_if(required, USED)
        .parsed(CenterUrl::parse);
    match &url {
        Ok(Some(url)) if url.is_https() => settings.describe(CENTER_URL, url),
        Ok(Some(url)) => settings.warn(
            CENTER_URL,
            url,
            "the credential and what Rail Center returns cross in the clear",
        ),
        _ => {}
    }
    let allow_http = settings
        .get(ALLOW_HTTP)
        .boolean()
        .map(|allow| allow.unwrap_or(false));
    if let Ok(allow_http) = allow_http {
        settings.describe(ALLOW_HTTP, allow_http);
    }
    let http = match &url {
        Ok(Some(url)) if !url.is_https() && !allow_http.unwrap_or(false) => {
            Err(settings.get(CENTER_URL).refuse(format_args!(
                "is http://, which sends the credential and what Rail Center returns in the clear; \
                 use https://, or set {ALLOW_HTTP}=true"
            )))
        }
        _ => Ok(()),
    };
    let token = read_token(settings, required);
    retired.into_iter().try_for_each(|retired| retired)?;
    let (url, allow_http, token) = (url?, allow_http?, token?);
    http?;
    Ok(url.zip(token).map(|(url, token)| RailCenterSettings {
        url,
        allow_http,
        token,
    }))
}

// `Some(None)` is no credential; `None` is bearer with no token, when it isn't required.
fn read_token(settings: &mut Settings, required: bool) -> Result<Option<Option<Secret>>, Refused> {
    // Unreadable: which credential is meant is unknown, so the token isn't judged.
    let mode = settings.get(AUTH_MODE).text()?;
    match mode
        .map(str::to_ascii_lowercase)
        .as_deref()
        .unwrap_or("none")
    {
        "none" => {
            if settings.get(AUTH_TOKEN).is_set() {
                let configured = if mode.is_some() {
                    "none"
                } else {
                    "unset, which is none"
                };
                return Err(settings.get(AUTH_MODE).refuse(format_args!(
                    "is {configured} and sends no credential, but {AUTH_TOKEN} is set; set \
                     {AUTH_MODE}=bearer to use it, or unset {AUTH_TOKEN}"
                )));
            }
            settings.describe(AUTH_MODE, "none");
            settings.describe_secret(AUTH_TOKEN, None);
            Ok(Some(None))
        }
        "bearer" => {
            settings.describe(AUTH_MODE, "bearer");
            let bearer = format!("{AUTH_MODE} is bearer");
            // Read as a secret, so nothing describes its text.
            let token = settings
                .get(AUTH_TOKEN)
                .required_if(required, &bearer)
                .secret()?;
            if let Some(token) = &token
                && let Err(reason) = check_token(token.expose())
            {
                return Err(settings.get(AUTH_TOKEN).refuse(reason));
            }
            settings.describe_secret(AUTH_TOKEN, token.as_ref());
            Ok(token.map(Some))
        }
        // Not quoted: a token pasted into the wrong variable would reach the log.
        _ => Err(settings.get(AUTH_MODE).refuse(format_args!(
            "must be none or bearer, and is something else ({} characters)",
            mode.unwrap_or_default().chars().count()
        ))),
    }
}

// Says where the token is wrong without quoting any of it.
fn check_token(token: &str) -> Result<(), String> {
    let expected = format!(
        "a Rail Center service token is {TOKEN_PREFIX} and {TOKEN_SECRET_CHARS} URL-safe base64 \
         characters"
    );
    let Some(secret) = token.strip_prefix(TOKEN_PREFIX) else {
        return Err(format!("doesn't start with {TOKEN_PREFIX}; {expected}"));
    };
    if let Some(offset) = token
        .char_indices()
        .skip(TOKEN_PREFIX.len())
        .find_map(|(offset, c)| {
            (!(c.is_ascii_alphanumeric() || c == '-' || c == '_')).then_some(offset)
        })
    {
        return Err(format!(
            "holds a character at offset {offset} that isn't URL-safe base64; {expected}"
        ));
    }
    if secret.len() != TOKEN_SECRET_CHARS {
        return Err(format!(
            "has {} characters after {TOKEN_PREFIX}; {expected}",
            secret.len()
        ));
    }
    Ok(())
}

/// Where Rail Center is: an http or https URL with a host, and nothing a route can't be appended
/// to (no credential, query or fragment). Displays without a trailing `/`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CenterUrl(Url);

impl CenterUrl {
    /// Refuses what the WHATWG parser and Python's `urlsplit` could read differently, so the host
    /// reached is the one an operator reads in the value. Never quotes the value, which may hold
    /// a credential.
    pub fn parse(text: &str) -> Result<Self, String> {
        if let Some((offset, _)) = text
            .char_indices()
            .find(|&(_, c)| !c.is_ascii_graphic() || c == '\\')
        {
            return Err(format!(
                "holds a space, a backslash or a character outside ASCII at offset {offset}"
            ));
        }
        let lowered = text.to_ascii_lowercase();
        let Some(rest) = ["https://", "http://"]
            .into_iter()
            .find_map(|scheme| lowered.starts_with(scheme).then(|| &text[scheme.len()..]))
        else {
            return Err("must start with http:// or https://".to_owned());
        };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        if authority.is_empty() {
            return Err("names no host".to_owned());
        }
        if authority.contains('@') {
            return Err(format!(
                "carries a credential, which isn't read from it; Rail Center's is set with \
                 {AUTH_MODE} and {AUTH_TOKEN}"
            ));
        }
        if authority.contains('%') {
            return Err("has a percent-encoded host".to_owned());
        }
        let mut url = Url::parse(text).map_err(|error| format!("is not a URL: {error}"))?;
        // WHATWG rewrites some hosts (`0x7f.1` is `127.0.0.1`), where `urlsplit` keeps the text.
        let host = match authority.find(']') {
            Some(end) if authority.starts_with('[') => &authority[..=end],
            _ => authority.split(':').next().unwrap_or_default(),
        };
        if url.host_str() != Some(host.to_ascii_lowercase().as_str()) {
            return Err("has a host the WHATWG parser reads as another".to_owned());
        }
        if url.query().is_some_and(|query| !query.is_empty())
            || url.fragment().is_some_and(|fragment| !fragment.is_empty())
        {
            return Err(
                "must carry no query or fragment; it's the base each route is appended to"
                    .to_owned(),
            );
        }
        url.set_query(None);
        url.set_fragment(None);
        let path = url.path().trim_end_matches('/').to_owned();
        url.set_path(&path);
        Ok(Self(url))
    }

    pub fn is_https(&self) -> bool {
        self.0.scheme() == "https"
    }

    /// The URL of `route` (starting with `/`) under this base, with `query` encoded.
    pub fn endpoint(&self, route: &str, query: &[(&str, &str)]) -> Url {
        debug_assert!(route.starts_with('/'), "{route}");
        let mut url = self.0.clone();
        url.set_path(&format!("{}{route}", self.0.path().trim_end_matches('/')));
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query);
        }
        url
    }
}

impl fmt::Display for CenterUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.as_str().trim_end_matches('/'))
    }
}
