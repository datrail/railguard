use std::error::Error as _;
use std::fmt;
use std::io::{self, Write};
use std::sync::Arc;
use std::time::Duration;

use flate2::write::GzDecoder;
use reqwest::header::{
    ACCEPT, ACCEPT_ENCODING, AUTHORIZATION, CACHE_CONTROL, CONTENT_ENCODING, HeaderMap, HeaderValue,
};
use reqwest::{StatusCode, Url, redirect};
use tokio::time::timeout;

use crate::rail_center::{AUTH_MODE, AUTH_TOKEN, CenterUrl, RailCenterSettings};

/// What one caller allows a fetch.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// How long the fetch may take, its whole body included.
    pub deadline: Duration,
    /// How long Rail Center may send nothing, for the response and then for each chunk of its
    /// body: one gone quiet is given up on before the deadline.
    pub silence: Duration,
    /// The largest body read, counted after decoding.
    pub max_bytes: usize,
}

/// A response, whatever its status: what each status means is the caller's.
#[derive(Debug)]
pub struct Response {
    pub status: StatusCode,
    pub body: Vec<u8>,
}

/// Why a fetch got no response: never the credential, route or query (a TLS error names the host).
#[derive(Debug)]
pub enum FetchError {
    Deadline(Duration),
    Silent(Duration),
    TooLarge { max_bytes: usize },
    Encoding(String),
    BadGzip(String),
    Transport(String),
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Deadline(deadline) => write!(f, "the fetch ran past its {deadline:?} deadline"),
            Self::Silent(silence) => write!(f, "Rail Center was silent for {silence:?}"),
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
/// from the environment, no redirects, and a bounded body. A clone shares the connection pool.
#[derive(Clone)]
pub struct RailCenterClient {
    http: reqwest::Client,
    url: CenterUrl,
}

impl RailCenterClient {
    pub fn new(settings: &RailCenterSettings) -> Result<Self, String> {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        // Rail Center's frontend may compress of its own accord; gzip is decoded here, capped.
        headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip, identity"));
        // A cache in between handing back what Rail Center has since changed would restart the
        // age on a withdrawn key, or keep a policy in force after it changed.
        headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        if let Some(token) = &settings.token {
            let mut value = HeaderValue::from_str(&format!("Bearer {}", token.expose()))
                .expect("the settings reader allows only URL-safe base64 in a token");
            // Kept out of reqwest's own Debug, as its `bearer_auth` does.
            value.set_sensitive(true);
            headers.insert(AUTHORIZATION, value);
        }
        let http = reqwest::Client::builder()
            .tls_backend_preconfigured(tls_config())
            .no_proxy()
            .redirect(redirect::Policy::none())
            .pool_idle_timeout(settings.idle_timeout)
            .default_headers(headers)
            .build()
            .map_err(|error| format!("the HTTP client can't be built: {}", describe(error)))?;
        Ok(Self {
            http,
            url: settings.url.clone(),
        })
    }

    pub(crate) fn is_https(&self) -> bool {
        self.url.is_https()
    }

    /// `GET` `route`, in one attempt: retrying is the caller's.
    pub async fn get(
        &self,
        route: &str,
        query: &[(&str, &str)],
        limits: Limits,
    ) -> Result<Response, FetchError> {
        let url = self.url.endpoint(route, query);
        timeout(limits.deadline, self.fetch(url, limits))
            .await
            .unwrap_or(Err(FetchError::Deadline(limits.deadline)))
    }

    async fn fetch(&self, url: Url, limits: Limits) -> Result<Response, FetchError> {
        let mut response = timeout(limits.silence, self.http.get(url).send())
            .await
            .map_err(|_| FetchError::Silent(limits.silence))?
            .map_err(|error| FetchError::Transport(describe(error)))?;
        if response
            .content_length()
            .is_some_and(|length| length > limits.max_bytes as u64)
        {
            return Err(FetchError::TooLarge {
                max_bytes: limits.max_bytes,
            });
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
        while let Some(chunk) = timeout(limits.silence, response.chunk())
            .await
            .map_err(|_| FetchError::Silent(limits.silence))?
            .map_err(|error| FetchError::Transport(describe(error)))?
        {
            body.write_all(&chunk)
                .map_err(|error| read_error(error, limits))?;
        }
        let body = body.finish().map_err(|error| read_error(error, limits))?;
        Ok(Response { status, body })
    }
}

/// The body of a 200, or why the answer has none.
pub(crate) fn read_ok(answer: Result<Response, FetchError>) -> Result<Vec<u8>, String> {
    let response = answer.map_err(|error| error.to_string())?;
    match response.status {
        StatusCode::OK => Ok(response.body),
        // Unlike Rail Center having a bad minute, this lasts until the configuration changes.
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(format!(
            "Rail Center refused the credential ({}); every fetch fails this way until \
             {AUTH_MODE} and {AUTH_TOKEN} are fixed",
            response.status
        )),
        status => Err(format!("Rail Center answered {status}")),
    }
}

// Every value of every `Content-Encoding` line, so a second encoding can't hide behind a first.
fn is_gzipped(headers: &HeaderMap) -> Result<bool, FetchError> {
    let mut gzipped = false;
    for line in headers.get_all(CONTENT_ENCODING) {
        let line = String::from_utf8_lossy(line.as_bytes());
        for encoding in line.split(',').map(str::trim) {
            if encoding.eq_ignore_ascii_case("gzip") && !gzipped {
                gzipped = true;
            } else if !encoding.eq_ignore_ascii_case("identity") {
                return Err(FetchError::Encoding(line.into_owned()));
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

fn read_error(error: io::Error, limits: Limits) -> FetchError {
    if error.kind() == io::ErrorKind::FileTooLarge {
        FetchError::TooLarge {
            max_bytes: limits.max_bytes,
        }
    } else {
        FetchError::BadGzip(error.to_string())
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

// The error and its causes, without the URL; the credential is only ever in a header.
fn describe(error: reqwest::Error) -> String {
    let error = error.without_url();
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}
