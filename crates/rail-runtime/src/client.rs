use std::error::Error as _;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use rail_policy::sanitize;
use reqwest::header::{
    ACCEPT, AUTHORIZATION, CONTENT_ENCODING, HeaderMap, HeaderName, HeaderValue,
};
use reqwest::{StatusCode, Url, redirect};
use tokio::time::timeout;

use crate::rail_center::{CenterUrl, RailCenterSettings};

/// What one caller allows a fetch.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// How long one attempt may wait for the response, and then for each chunk of its body.
    pub attempt_timeout: Duration,
    /// How long the whole fetch may take, its retry included.
    pub deadline: Duration,
    /// The largest body read, counted after decoding.
    pub max_bytes: usize,
}

/// A response, whatever its status: what each status means is the caller's.
#[derive(Debug)]
pub struct Response {
    pub status: StatusCode,
    pub body: Vec<u8>,
}

/// Why a fetch got no response. Never carries the credential or a URL.
#[derive(Debug)]
pub enum FetchError {
    Deadline(Duration),
    Timeout(Duration),
    TooLarge { max_bytes: usize },
    Encoding(String),
    Transport(String),
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Deadline(deadline) => write!(f, "the fetch ran past its {deadline:?} deadline"),
            Self::Timeout(timeout) => write!(f, "Rail Center was silent for {timeout:?}"),
            Self::TooLarge { max_bytes } => {
                write!(f, "the response is past the {max_bytes} bytes read")
            }
            Self::Encoding(encoding) => write!(
                f,
                "the response is {encoding}-encoded, and only gzip and identity are read"
            ),
            Self::Transport(reason) => f.write_str(reason),
        }
    }
}

impl std::error::Error for FetchError {}

/// The one client every Rail Center call goes through: rustls with compiled-in roots, no proxy
/// from the environment, no redirects, and a bounded body.
pub struct RailCenterClient {
    http: reqwest::Client,
    url: CenterUrl,
}

impl RailCenterClient {
    pub fn new(settings: &RailCenterSettings) -> Result<Self, String> {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, sendable(&ACCEPT, "application/json")?);
        if let Some(token) = &settings.token {
            let value = sendable(&AUTHORIZATION, &format!("Bearer {}", token.expose()))?;
            headers.insert(AUTHORIZATION, value);
        }
        let http = reqwest::Client::builder()
            .tls_backend_preconfigured(tls_config())
            .no_proxy()
            .redirect(redirect::Policy::none())
            .default_headers(headers)
            .build()
            .map_err(|error| format!("the HTTP client can't be built: {}", describe(error)))?;
        Ok(Self {
            http,
            url: settings.url.clone(),
        })
    }

    /// `GET` `route` under Rail Center's URL. A request that fails before any response is retried
    /// once, since the first one after a long idle may meet a pooled connection the peer closed.
    pub async fn get(
        &self,
        route: &str,
        query: &[(&str, &str)],
        limits: Limits,
    ) -> Result<Response, FetchError> {
        let url = self.url.endpoint(route, query);
        let fetch = async {
            match self.attempt(&url, limits).await {
                Err(Attempt::Unsent(_)) => self.attempt(&url, limits).await,
                outcome => outcome,
            }
            .map_err(FetchError::from)
        };
        timeout(limits.deadline, fetch)
            .await
            .unwrap_or(Err(FetchError::Deadline(limits.deadline)))
    }

    async fn attempt(&self, url: &Url, limits: Limits) -> Result<Response, Attempt> {
        let mut response = timeout(limits.attempt_timeout, self.http.get(url.clone()).send())
            .await
            .map_err(|_| FetchError::Timeout(limits.attempt_timeout))?
            .map_err(|error| Attempt::Unsent(describe(error)))?;
        if response
            .content_length()
            .is_some_and(|length| length > limits.max_bytes as u64)
        {
            return Err(FetchError::TooLarge {
                max_bytes: limits.max_bytes,
            }
            .into());
        }
        // Decoding removes the header, so one left is an encoding nothing here decodes.
        for encoding in response.headers().get_all(CONTENT_ENCODING) {
            let encoding = String::from_utf8_lossy(encoding.as_bytes());
            if !encoding.trim().eq_ignore_ascii_case("identity") {
                return Err(FetchError::Encoding(sanitize(&encoding).to_string()).into());
            }
        }
        let status = response.status();
        let mut body = Vec::new();
        // Each chunk is decoded, so the cap counts what's held, whatever the compression ratio.
        while let Some(chunk) = timeout(limits.attempt_timeout, response.chunk())
            .await
            .map_err(|_| FetchError::Timeout(limits.attempt_timeout))?
            .map_err(|error| FetchError::Transport(describe(error)))?
        {
            if body.len() + chunk.len() > limits.max_bytes {
                return Err(FetchError::TooLarge {
                    max_bytes: limits.max_bytes,
                }
                .into());
            }
            body.extend_from_slice(&chunk);
        }
        Ok(Response { status, body })
    }
}

enum Attempt {
    Unsent(String),
    Failed(FetchError),
}

impl From<FetchError> for Attempt {
    fn from(error: FetchError) -> Self {
        Self::Failed(error)
    }
}

impl From<Attempt> for FetchError {
    fn from(attempt: Attempt) -> Self {
        match attempt {
            Attempt::Unsent(reason) => Self::Transport(reason),
            Attempt::Failed(error) => error,
        }
    }
}

fn tls_config() -> rustls::ClientConfig {
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("ring supports the default protocol versions")
        .with_root_certificates(roots)
        .with_no_client_auth()
}

// A header value h11 would refuse quoting it is refused here, naming only the header.
fn sendable(name: &HeaderName, value: &str) -> Result<HeaderValue, String> {
    if let Some(offset) = value
        .bytes()
        .position(|byte| !(0x20..=0x7e).contains(&byte))
    {
        return Err(format!(
            "the {name} header holds a byte at offset {offset} that can't go in a header value"
        ));
    }
    if value.trim() != value {
        return Err(format!("the {name} header is padded with whitespace"));
    }
    HeaderValue::from_str(value).map_err(|_| format!("the {name} header can't be sent"))
}

// The error and its causes, without the URL; the credential is only ever in a header.
fn describe(error: reqwest::Error) -> String {
    let error = error.without_url();
    let mut text = sanitize(&error.to_string()).to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&sanitize(&cause.to_string()).to_string());
        source = cause.source();
    }
    text
}
