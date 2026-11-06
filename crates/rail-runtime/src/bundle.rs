use std::future::Future;
use std::time::Duration;

use rail_policy::sanitize;

use crate::client::{FetchError, Limits, RailCenterClient, Response};
use crate::config::{Refused, Settings};
use crate::holder::MIN_REFRESH_INTERVAL;

const ROUTE: &str = "/v1/policy-bundle";

// The deadline covers a whole 4 MiB body on a slow link; a body past that is a fault, not a large
// tenant, whose chain is a few dozen policies. A Rail Center gone quiet is given up on sooner.
const LIMITS: Limits = Limits {
    deadline: Duration::from_secs(30),
    silence: Some(Duration::from_secs(5)),
    max_bytes: 4 * 1024 * 1024,
};

pub(crate) const GATEWAY_SLUG: &str = "RAIL_GATEWAY_SLUG";
pub(crate) const BUNDLE_REFRESH_SECONDS: &str = "RAIL_GATEWAY_BUNDLE_REFRESH_SECONDS";

// Rail Center's bound on the `gateway` query (`SLUG_MAX_LENGTH`), in characters.
const MAX_GATEWAY_SLUG_CHARS: usize = 64;

// A rule an operator changes reaches every gateway within a minute.
const DEFAULT_REFRESH_INTERVAL: Duration = Duration::from_secs(60);

// A day: longer is surely a mistake.
const MAX_REFRESH_INTERVAL: Duration = Duration::from_secs(86_400);

/// How this gateway's policy bundle is fetched and kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleSettings {
    /// Which gateway this is, and so whose bundle Rail Center builds.
    pub gateway_slug: String,
    pub refresh_interval: Duration,
}

/// The bundle's settings, every one set judged, `None` if the slug is missing; it's required if
/// `required`, which is whether Rail Center is used. Described only when read whole.
pub fn read_bundle_settings(
    settings: &mut Settings,
    required: bool,
) -> Result<Option<BundleSettings>, Refused> {
    let gateway_slug = settings
        .get(GATEWAY_SLUG)
        .required_if(required)
        .max_chars(MAX_GATEWAY_SLUG_CHARS)
        .parsed(parse_gateway_slug);
    let refresh = settings
        .get(BUNDLE_REFRESH_SECONDS)
        .parsed(|text| match text.parse::<u64>() {
            // The holder never refreshes faster, so a lower interval would be silently raised.
            Ok(seconds)
                if (MIN_REFRESH_INTERVAL..=MAX_REFRESH_INTERVAL)
                    .contains(&Duration::from_secs(seconds)) =>
            {
                Ok(Duration::from_secs(seconds))
            }
            _ => Err(format!(
                "must be a whole number of seconds, from {} to {}, got: {}",
                MIN_REFRESH_INTERVAL.as_secs(),
                MAX_REFRESH_INTERVAL.as_secs(),
                sanitize(text)
            )),
        })
        .map(|refresh| refresh.unwrap_or(DEFAULT_REFRESH_INTERVAL));
    let (gateway_slug, refresh_interval) = (gateway_slug?, refresh?);
    let bundle = gateway_slug.map(|gateway_slug| BundleSettings {
        gateway_slug,
        refresh_interval,
    });
    if let Some(bundle) = &bundle {
        settings.describe(GATEWAY_SLUG, &bundle.gateway_slug);
        settings.describe(BUNDLE_REFRESH_SECONDS, bundle.refresh_interval.as_secs());
    }
    Ok(bundle)
}

// What Rail Center accepts as a slug (`^[A-Za-z0-9_-]+$`): any other it would refuse each fetch.
fn parse_gateway_slug(text: &str) -> Result<String, String> {
    let valid = |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_');
    if !text.bytes().all(valid) {
        return Err(format!(
            "must hold only ASCII letters, digits, - and _, as Rail Center's slugs do, got: {}",
            sanitize(text)
        ));
    }
    Ok(text.to_owned())
}

/// Where bundles come from: Rail Center, or a test's script.
pub trait BundleSource: Send + Sync + 'static {
    /// One fetch, whatever its status: what each means is the holder's.
    fn fetch(&self) -> impl Future<Output = Result<Response, FetchError>> + Send;
}

/// This gateway's bundle, fetched from Rail Center.
pub struct RailCenterBundles {
    pub client: RailCenterClient,
    pub gateway_slug: String,
}

impl BundleSource for RailCenterBundles {
    async fn fetch(&self) -> Result<Response, FetchError> {
        let query = [("gateway", self.gateway_slug.as_str())];
        self.client.get(ROUTE, &query, LIMITS).await
    }
}
