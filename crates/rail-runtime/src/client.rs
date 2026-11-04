use std::error::Error as _;
use std::fmt;
use std::io::{self, Write};
use std::sync::Arc;
use std::time::Duration;

use flate2::write::GzDecoder;
use rail_policy::sanitize;
use reqwest::header::{
    ACCEPT, ACCEPT_ENCODING, AUTHORIZATION, CONTENT_ENCODING, HeaderMap, HeaderName, HeaderValue,
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

/// Why a fetch got no response. Never carries the credential, a route or a query; a TLS name
/// mismatch names the host.
#[derive(Debug)]
pub enum FetchError {
    Deadline(Duration),
    Timeout(Duration),
    TooLarge { max_bytes: usize },
    Encoding(String),
    BadGzip(String),
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
                "the response's Content-Encoding is {encoding}, and only one gzip, or identity, \
                 is read"
            ),
            Self::BadGzip(reason) => write!(f, "the response's gzip doesn't decode: {reason}"),
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
        // Rail Center's frontend may compress of its own accord; gzip is decoded here, capped.
        headers.insert(
            ACCEPT_ENCODING,
            sendable(&ACCEPT_ENCODING, "gzip, identity")?,
        );
        if let Some(token) = &settings.token {
            let mut value = sendable(&AUTHORIZATION, &format!("Bearer {}", token.expose()))?;
            // Kept out of reqwest's own Debug, as its `bearer_auth` does.
            value.set_sensitive(true);
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

    /// `GET` `route` under Rail Center's URL, retried once if it fails before a response is read,
    /// as the first after a long idle may meet a pooled connection the peer closed.
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
        let gzipped = is_gzipped(response.headers())?;
        let status = response.status();
        let capped = Capped {
            body: Vec::new(),
            max_bytes: limits.max_bytes,
        };
        let mut body = if gzipped {
            Body::Gzip(GzDecoder::new(capped))
        } else {
            Body::Identity(capped)
        };
        while let Some(chunk) = timeout(limits.attempt_timeout, response.chunk())
            .await
            .map_err(|_| FetchError::Timeout(limits.attempt_timeout))?
            .map_err(|error| FetchError::Transport(describe(error)))?
        {
            body.write_all(&chunk)
                .map_err(|error| read_error(error, limits))?;
        }
        let body = body.finish().map_err(|error| read_error(error, limits))?;
        Ok(Response { status, body })
    }
}

// Every value of every `Content-Encoding` line, so a second encoding can't hide behind a first.
fn is_gzipped(headers: &HeaderMap) -> Result<bool, Attempt> {
    let mut gzipped = false;
    for line in headers.get_all(CONTENT_ENCODING) {
        let line = String::from_utf8_lossy(line.as_bytes());
        for encoding in line.split(',').map(str::trim) {
            if encoding.eq_ignore_ascii_case("gzip") && !gzipped {
                gzipped = true;
            } else if !encoding.eq_ignore_ascii_case("identity") {
                return Err(FetchError::Encoding(sanitize(&line).to_string()).into());
            }
        }
    }
    Ok(gzipped)
}

// What's held, refusing to grow past the cap: a small gzip chunk can't decode to more than it.
struct Capped {
    body: Vec<u8>,
    max_bytes: usize,
}

impl Write for Capped {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.body.len() + bytes.len() > self.max_bytes {
            return Err(io::Error::new(io::ErrorKind::FileTooLarge, "past the cap"));
        }
        self.body.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

enum Body {
    Identity(Capped),
    Gzip(GzDecoder<Capped>),
}

impl Body {
    fn write_all(&mut self, chunk: &[u8]) -> io::Result<()> {
        match self {
            Self::Identity(capped) => capped.write_all(chunk),
            Self::Gzip(decoder) => decoder.write_all(chunk),
        }
    }

    fn finish(self) -> io::Result<Vec<u8>> {
        match self {
            Self::Identity(capped) => Ok(capped.body),
            Self::Gzip(decoder) => decoder.finish().map(|capped| capped.body),
        }
    }
}

fn read_error(error: io::Error, limits: Limits) -> Attempt {
    if error.kind() == io::ErrorKind::FileTooLarge {
        FetchError::TooLarge {
            max_bytes: limits.max_bytes,
        }
        .into()
    } else {
        FetchError::BadGzip(error.to_string()).into()
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
