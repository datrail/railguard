//! The gateway role's callouts: each request judged whole, then forwarded without the agent's
//! withheld headers, or refused with the gateway's status and body.

use std::pin::Pin;
use std::sync::Arc;

use envoy_types::ext_authz::v3::CheckResponseExt;
use envoy_types::ext_authz::v3::pb::{
    Authorization, CheckRequest, CheckResponse, DeniedHttpResponse, OkHttpResponse,
};
use envoy_types::pb::envoy::config::core::v3::{HeaderValue, HeaderValueOption};
use envoy_types::pb::envoy::extensions::filters::http::ext_proc::v3::processing_mode::BodySendMode;
use envoy_types::pb::envoy::service::ext_proc::v3::body_mutation::Mutation;
use envoy_types::pb::envoy::service::ext_proc::v3::external_processor_server::ExternalProcessor;
use envoy_types::pb::envoy::service::ext_proc::v3::processing_request::Request;
use envoy_types::pb::envoy::service::ext_proc::v3::processing_response::Response;
use envoy_types::pb::envoy::service::ext_proc::v3::{
    BodyMutation, BodyResponse, CommonResponse, HeaderMutation, HeadersResponse, HttpBody,
    HttpHeaders, ImmediateResponse, ProcessingRequest, ProcessingResponse, StreamedBodyResponse,
    TrailersResponse,
};
use envoy_types::pb::envoy::r#type::v3::HttpStatus;
use rail_runtime::{Answer, Call, Gateway, is_withheld_header, read_path};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::{Stream, StreamExt};
use tonic::{Status, Streaming};

// Envoy recommends full-duplex chunks of at most 64 KiB; GCP's load balancer takes 128 kB.
const CHUNK_BYTES: usize = 64 * 1024;

// Set by Envoy when `allow_partial_message` cut the body at `max_request_bytes`.
const PARTIAL_BODY: &str = "x-envoy-auth-partial-body";

/// Both callouts in the gateway role; with no gateway, Rail Center is disabled and every request
/// forwarded.
#[derive(Clone)]
pub struct GatewayCallouts {
    pub gateway: Option<Arc<Gateway>>,
    /// The largest body judged: past it, a streamed body is refused as it arrives.
    pub max_body_bytes: usize,
}

/// ext_authz: judges the request with the body Envoy sends (`with_request_body`).
#[tonic::async_trait]
impl Authorization for GatewayCallouts {
    async fn check(
        &self,
        request: tonic::Request<CheckRequest>,
    ) -> Result<tonic::Response<CheckResponse>, Status> {
        let http = request
            .into_inner()
            .attributes
            .and_then(|attributes| attributes.request)
            .and_then(|request| request.http)
            .unwrap_or_default();
        // Envoy sends the headers in one of the two, `header_map` when asked for raw headers.
        let sent = http.headers.into_iter().chain(
            http.header_map
                .into_iter()
                .flat_map(|map| map.headers)
                .map(|header| (header.key.clone(), read_value(&header))),
        );
        let request = read_request(sent);
        let body = if http.raw_body.is_empty() {
            http.body.into_bytes()
        } else {
            http.raw_body
        };
        let removed = withheld(&request.lines);
        let partial = request
            .lines
            .iter()
            .any(|(name, value)| name.eq_ignore_ascii_case(PARTIAL_BODY) && value == "true");
        let short = usize::try_from(http.size).is_ok_and(|size| body.len() < size);
        // An MCP call always has a body: none, where the size isn't known to be 0, is one not sent.
        let unsent = http.method == "POST" && body.is_empty() && http.size != 0;
        let answer = match &self.gateway {
            None => Answer::Forward,
            Some(_) if unsent => {
                tracing::error!(
                    "refused unjudged: Envoy sent no body; its ext_authz filter needs with_request_body"
                );
                Answer::CannotApply
            }
            // A body cut short could read as another call, so it's never judged as whole.
            Some(_) if partial || short => {
                tracing::warn!(
                    bytes = body.len(),
                    "refused unjudged: Envoy sent part of the body"
                );
                Answer::TooLarge
            }
            Some(gateway) => {
                let call = Call {
                    path: read_path(&http.path),
                    method: http.method,
                    body,
                    headers: request.lines,
                };
                gateway.handle(call).await.0
            }
        };
        let response = match answer.refusal() {
            None => {
                let mut response = CheckResponse::with_status(Status::ok(""));
                response.set_http_response(OkHttpResponse {
                    headers_to_remove: removed,
                    ..Default::default()
                });
                response
            }
            Some((status, body)) => {
                let mut response = CheckResponse::with_status(Status::permission_denied(""));
                response.set_http_response(DeniedHttpResponse {
                    status: Some(HttpStatus {
                        code: i32::from(status),
                    }),
                    headers: vec![json_content_type(|value| (value.to_owned(), Vec::new()))],
                    body: body.to_owned(),
                });
                response
            }
        };
        Ok(tonic::Response::new(response))
    }
}

type ProcessStream = Pin<Box<dyn Stream<Item = Result<ProcessingResponse, Status>> + Send>>;

/// ext_proc: gathers the headers and the whole body, then answers them in order, or refuses.
#[tonic::async_trait]
impl ExternalProcessor for GatewayCallouts {
    type ProcessStream = ProcessStream;

    async fn process(
        &self,
        request: tonic::Request<Streaming<ProcessingRequest>>,
    ) -> Result<tonic::Response<ProcessStream>, Status> {
        let mut inbound = request.into_inner();
        let (replies, outbound) = mpsc::channel(16);
        let mut exchange = Exchange {
            callouts: self.clone(),
            mode: None,
            first: true,
            pending: None,
            done: false,
        };
        // Several answers can follow one message, so they go through a channel.
        tokio::spawn(async move {
            while let Some(Ok(message)) = inbound.next().await {
                let sent = match exchange.on(message).await {
                    Ok(answers) => send(&replies, answers).await,
                    Err(status) => {
                        let _ = replies.send(Err(status)).await;
                        Err(())
                    }
                };
                if sent.is_err() {
                    return;
                }
            }
        });
        Ok(tonic::Response::new(Box::pin(ReceiverStream::new(
            outbound,
        ))))
    }
}

// What one message is answered with: answers ready now, or a full-duplex request forwarded,
// whose body goes back a chunk at a time, so it's never copied whole.
enum Answers {
    Now(Vec<Response>),
    BodyBack {
        removed: Vec<String>,
        body: Vec<u8>,
        trailers: bool,
    },
}

async fn send(
    replies: &mpsc::Sender<Result<ProcessingResponse, Status>>,
    answers: Answers,
) -> Result<(), ()> {
    let send_one = |response| replies.send(Ok(reply(response)));
    match answers {
        Answers::Now(responses) => {
            for response in responses {
                send_one(response).await.map_err(|_| ())?;
            }
        }
        Answers::BodyBack {
            removed,
            body,
            trailers,
        } => {
            send_one(headers_response(removed)).await.map_err(|_| ())?;
            // With trailers, they end the stream; without, the last chunk does, even an empty one.
            let mut chunks: Vec<&[u8]> = body.chunks(CHUNK_BYTES).collect();
            if chunks.is_empty() && !trailers {
                chunks.push(&[]);
            }
            let last = chunks.len().saturating_sub(1);
            for (index, chunk) in chunks.into_iter().enumerate() {
                let chunk = StreamedBodyResponse {
                    body: chunk.to_vec(),
                    end_of_stream: !trailers && index == last,
                    ..Default::default()
                };
                send_one(streamed(chunk)).await.map_err(|_| ())?;
            }
            if trailers {
                send_one(Response::RequestTrailers(TrailersResponse::default()))
                    .await
                    .map_err(|_| ())?;
            }
        }
    }
    Ok(())
}

// One request's stream: what's gathered of it, and how Envoy sends its body.
struct Exchange {
    callouts: GatewayCallouts,
    mode: Option<BodySendMode>,
    first: bool,
    pending: Option<Pending>,
    // Answered whole, by a refusal or the last of its phases: anything after is only acknowledged.
    done: bool,
}

struct Pending {
    path: String,
    method: String,
    headers: Vec<(String, String)>,
    removed: Vec<String>,
    body: Vec<u8>,
}

impl Exchange {
    async fn on(&mut self, message: ProcessingRequest) -> Result<Answers, Status> {
        if std::mem::take(&mut self.first) {
            if message.observability_mode {
                tracing::error!(
                    "ext_proc in observability mode: its answers are ignored, so nothing could be enforced"
                );
                return Err(Status::failed_precondition(
                    "rail-envoy's gateway enforces, so it can't run in observability mode",
                ));
            }
            self.mode = message
                .protocol_config
                .and_then(|config| BodySendMode::try_from(config.request_body_mode).ok());
        }
        let request = message
            .request
            .ok_or_else(|| Status::invalid_argument("the message names no phase"))?;
        Ok(match request {
            Request::RequestHeaders(headers) => self.on_headers(headers).await,
            Request::RequestBody(body) => self.on_body(body).await,
            Request::RequestTrailers(_) => self.on_trailers().await,
            Request::ResponseHeaders(_) => {
                Answers::Now(vec![Response::ResponseHeaders(HeadersResponse::default())])
            }
            Request::ResponseBody(_) => {
                Answers::Now(vec![Response::ResponseBody(BodyResponse::default())])
            }
            Request::ResponseTrailers(_) => {
                Answers::Now(vec![
                    Response::ResponseTrailers(TrailersResponse::default()),
                ])
            }
        })
    }

    async fn on_headers(&mut self, headers: HttpHeaders) -> Answers {
        let sent = headers
            .headers
            .iter()
            .flat_map(|map| &map.headers)
            .map(|header| (header.key.clone(), read_value(header)));
        let request = read_request(sent);
        let removed = withheld(&request.lines);
        // Reserved once, when its size is known and within the cap.
        let capacity = request
            .lines
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, value)| value.parse::<usize>().ok())
            .filter(|&length| length <= self.callouts.max_body_bytes)
            .unwrap_or(0);
        let pending = Pending {
            path: request.path,
            method: request.method,
            headers: request.lines,
            removed,
            body: Vec::with_capacity(capacity),
        };
        if headers.end_of_stream {
            self.done = true;
            return self.judge(pending, Ending::Headers).await;
        }
        if self.callouts.gateway.is_none() {
            // Disabled: nothing to judge, so nothing to gather.
            return Answers::Now(vec![headers_response(pending.removed)]);
        }
        match self.mode {
            Some(BodySendMode::Buffered) => {
                let answer = headers_response(pending.removed.clone());
                self.pending = Some(pending);
                Answers::Now(vec![answer])
            }
            Some(BodySendMode::FullDuplexStreamed) => {
                self.pending = Some(pending);
                Answers::Now(Vec::new())
            }
            mode => {
                let mode = mode.map_or("unknown", |mode| mode.as_str_name());
                tracing::error!(
                    mode,
                    "refused unjudged: ext_proc must send the body BUFFERED or FULL_DUPLEX_STREAMED"
                );
                self.refuse(Answer::CannotApply)
            }
        }
    }

    async fn on_body(&mut self, body: HttpBody) -> Answers {
        if self.done {
            return Answers::Now(Vec::new());
        }
        let Some(pending) = self.pending.as_mut() else {
            if self.callouts.gateway.is_some() {
                tracing::error!("refused unjudged: ext_proc sent a body before its headers");
                return self.refuse(Answer::CannotApply);
            }
            // Disabled: passed on as it came.
            return Answers::Now(vec![pass_body(body, self.mode)]);
        };
        pending.body.extend_from_slice(&body.body);
        if pending.body.len() > self.callouts.max_body_bytes {
            tracing::warn!(
                bytes = pending.body.len(),
                limit = self.callouts.max_body_bytes,
                "refused unjudged: the body is past RAIL_MAX_BODY_MB"
            );
            return self.refuse(Answer::TooLarge);
        }
        // Buffered, the one message is the whole body, trailers or not.
        if !body.end_of_stream && self.mode != Some(BodySendMode::Buffered) {
            return Answers::Now(Vec::new());
        }
        let pending = self.pending.take().expect("held above");
        self.done = true;
        self.judge(pending, Ending::Body).await
    }

    // The trailers end a request still gathered: full duplex's body, or no body at all.
    async fn on_trailers(&mut self) -> Answers {
        match self.pending.take() {
            Some(pending) if !self.done => {
                self.done = true;
                self.judge(pending, Ending::Trailers).await
            }
            _ => Answers::Now(vec![Response::RequestTrailers(TrailersResponse::default())]),
        }
    }

    fn refuse(&mut self, answer: Answer) -> Answers {
        self.done = true;
        self.pending = None;
        Answers::Now(vec![immediate(answer)])
    }

    // The answers to every phase held, in order, or a refusal.
    async fn judge(&self, pending: Pending, ending: Ending) -> Answers {
        let removed = pending.removed;
        let Some(gateway) = &self.callouts.gateway else {
            return Answers::Now(vec![headers_response(removed)]);
        };
        let call = Call {
            path: pending.path,
            method: pending.method,
            body: pending.body,
            headers: pending.headers,
        };
        let (answer, call) = gateway.handle(call).await;
        if answer != Answer::Forward {
            return Answers::Now(vec![immediate(answer)]);
        }
        let full_duplex = self.mode == Some(BodySendMode::FullDuplexStreamed);
        match ending {
            Ending::Headers => Answers::Now(vec![headers_response(removed)]),
            Ending::Body | Ending::Trailers if full_duplex => Answers::BodyBack {
                removed,
                body: call.body,
                trailers: ending == Ending::Trailers,
            },
            // Buffered, its headers were answered when they came.
            Ending::Body => Answers::Now(vec![Response::RequestBody(BodyResponse::default())]),
            Ending::Trailers => {
                Answers::Now(vec![Response::RequestTrailers(TrailersResponse::default())])
            }
        }
    }
}

// What ended the request: its headers, its last body chunk, or its trailers.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ending {
    Headers,
    Body,
    Trailers,
}

fn reply(response: Response) -> ProcessingResponse {
    ProcessingResponse {
        response: Some(response),
        ..Default::default()
    }
}

fn streamed(chunk: StreamedBodyResponse) -> Response {
    Response::RequestBody(BodyResponse {
        response: Some(CommonResponse {
            body_mutation: Some(BodyMutation {
                mutation: Some(Mutation::StreamedResponse(chunk)),
            }),
            ..Default::default()
        }),
    })
}

// With nothing gathered, a chunk goes on as it came: echoed in full duplex, else continued.
fn pass_body(body: HttpBody, mode: Option<BodySendMode>) -> Response {
    if mode == Some(BodySendMode::FullDuplexStreamed) {
        streamed(StreamedBodyResponse {
            body: body.body,
            end_of_stream: body.end_of_stream,
            ..Default::default()
        })
    } else {
        Response::RequestBody(BodyResponse::default())
    }
}

fn headers_response(removed: Vec<String>) -> Response {
    Response::RequestHeaders(HeadersResponse {
        response: Some(CommonResponse {
            header_mutation: Some(HeaderMutation {
                remove_headers: removed,
                ..Default::default()
            }),
            ..Default::default()
        }),
    })
}

fn immediate(answer: Answer) -> Response {
    let (status, body) = answer
        .refusal()
        .expect("only a refusal is answered at once");
    Response::ImmediateResponse(ImmediateResponse {
        status: Some(HttpStatus {
            code: i32::from(status),
        }),
        headers: Some(HeaderMutation {
            set_headers: vec![json_content_type(|value| {
                (String::new(), value.as_bytes().to_vec())
            })],
            ..Default::default()
        }),
        body: body.as_bytes().to_vec(),
        ..Default::default()
    })
}

// ext_authz reads a header's `value`, ext_proc its `raw_value`.
fn json_content_type(encode: impl Fn(&str) -> (String, Vec<u8>)) -> HeaderValueOption {
    let (value, raw_value) = encode("application/json");
    HeaderValueOption {
        header: Some(HeaderValue {
            key: "content-type".to_owned(),
            value,
            raw_value,
        }),
        ..Default::default()
    }
}

struct ReadRequest {
    path: String,
    method: String,
    // Every header line but the pseudo-headers, `:authority` as `host`.
    lines: Vec<(String, String)>,
}

// One rule for both callouts: rules read `host` whether the request came as HTTP/1 or HTTP/2.
fn read_request(sent: impl Iterator<Item = (String, String)>) -> ReadRequest {
    let mut request = ReadRequest {
        path: "/".to_owned(),
        method: String::new(),
        lines: Vec::new(),
    };
    let mut authority = None;
    for (key, value) in sent {
        match key.as_str() {
            ":path" => request.path = read_path(&value),
            ":method" => request.method = value,
            ":authority" => authority = Some(value),
            key if key.starts_with(':') => {}
            _ => request.lines.push((key, value)),
        }
    }
    let has_host = request
        .lines
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("host"));
    if let (Some(authority), false) = (authority, has_host) {
        request.lines.push(("host".to_owned(), authority));
    }
    request
}

// Envoy sends a value in `raw_value`, or in `value` when not asked for raw headers.
fn read_value(header: &HeaderValue) -> String {
    match header.raw_value.as_slice() {
        [] => header.value.clone(),
        raw => String::from_utf8_lossy(raw).into_owned(),
    }
}

// The agent's `x-rail*` and `_` headers, which never reach the upstream.
fn withheld(headers: &[(String, String)]) -> Vec<String> {
    let mut names: Vec<String> = headers
        .iter()
        .map(|(name, _)| name.to_ascii_lowercase())
        .filter(|name| is_withheld_header(name))
        .collect();
    names.sort();
    names.dedup();
    names
}
