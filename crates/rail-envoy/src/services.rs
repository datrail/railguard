use std::pin::Pin;
use std::sync::Arc;

use envoy_types::ext_authz::v3::CheckResponseExt;
use envoy_types::ext_authz::v3::pb::{Authorization, CheckRequest, CheckResponse, OkHttpResponse};
use envoy_types::pb::envoy::service::ext_proc::v3::external_processor_server::ExternalProcessor;
use envoy_types::pb::envoy::service::ext_proc::v3::processing_request::Request;
use envoy_types::pb::envoy::service::ext_proc::v3::processing_response::Response;
use envoy_types::pb::envoy::service::ext_proc::v3::{
    BodyResponse, CommonResponse, HeaderMutation, HeadersResponse, ProcessingRequest,
    ProcessingResponse, TrailersResponse,
};
use rail_runtime::{ProtectedHosts, TicketHolder, XRailHeaders};
use tokio_stream::{Stream, StreamExt};
use tonic::{Status, Streaming};

use crate::xrail::{as_raw_value, as_value};

/// Both callouts, which change a request alike: one clone serves each.
#[derive(Clone)]
pub struct Callouts {
    pub tickets: Option<Arc<TicketHolder>>,
    pub protected_hosts: Arc<ProtectedHosts>,
}

/// ext_authz: allows every request, with its x-rail headers and, to a protected host, its route.
#[tonic::async_trait]
impl Authorization for Callouts {
    async fn check(
        &self,
        request: tonic::Request<CheckRequest>,
    ) -> Result<tonic::Response<CheckResponse>, Status> {
        let http = request
            .get_ref()
            .attributes
            .as_ref()
            .and_then(|attributes| attributes.request.as_ref())
            .and_then(|request| request.http.as_ref());
        // Envoy sends the headers in one of the two, `header_map` when asked for raw headers.
        let sent = http.into_iter().flat_map(|http| {
            http.headers.keys().map(String::as_str).chain(
                http.header_map
                    .iter()
                    .flat_map(|map| map.headers.iter().map(|header| header.key.as_str())),
            )
        });
        // Envoy sends `:authority` as `host`.
        let authority = http.map_or("", |http| http.host.as_str());
        let ticket = self.tickets.as_deref().map(TicketHolder::current);
        let headers = XRailHeaders::of(ticket, authority, &self.protected_hosts, sent);
        let mut response = CheckResponse::with_status(Status::ok(""));
        response.set_http_response(OkHttpResponse {
            headers: as_value(&headers),
            headers_to_remove: headers.remove,
            ..Default::default()
        });
        Ok(tonic::Response::new(response))
    }
}

type ProcessStream = Pin<Box<dyn Stream<Item = Result<ProcessingResponse, Status>> + Send>>;

/// ext_proc: lets every phase continue, changing the request's headers as ext_authz does.
#[tonic::async_trait]
impl ExternalProcessor for Callouts {
    type ProcessStream = ProcessStream;

    async fn process(
        &self,
        request: tonic::Request<Streaming<ProcessingRequest>>,
    ) -> Result<tonic::Response<ProcessStream>, Status> {
        let tickets = self.tickets.clone();
        let protected_hosts = Arc::clone(&self.protected_hosts);
        let replies = request.into_inner().map(move |message| {
            let request = message?
                .request
                .ok_or_else(|| Status::invalid_argument("the message names no phase"))?;
            Ok(ProcessingResponse {
                response: Some(continue_after(
                    request,
                    tickets.as_deref(),
                    &protected_hosts,
                )),
                ..Default::default()
            })
        });
        Ok(tonic::Response::new(Box::pin(replies)))
    }
}

// Each phase gets its own kind of response (full-duplex bodies, never asked for, would need more).
fn continue_after(
    request: Request,
    tickets: Option<&TicketHolder>,
    protected_hosts: &ProtectedHosts,
) -> Response {
    match request {
        Request::RequestHeaders(request) => {
            let sent = request.headers.iter().flat_map(|map| &map.headers);
            // Envoy sends the value in `raw_value`, or in `value` when not asked for raw headers.
            let authority = sent
                .clone()
                .find(|header| header.key == ":authority")
                .map_or("", |header| match header.raw_value.as_slice() {
                    [] => header.value.as_str(),
                    raw => std::str::from_utf8(raw).unwrap_or_default(),
                });
            let headers = XRailHeaders::of(
                tickets.map(TicketHolder::current),
                authority,
                protected_hosts,
                sent.map(|header| header.key.as_str()),
            );
            Response::RequestHeaders(HeadersResponse {
                response: Some(CommonResponse {
                    header_mutation: Some(HeaderMutation {
                        set_headers: as_raw_value(&headers),
                        remove_headers: headers.remove,
                    }),
                    // The route is picked again, from `x-rail-upstream` as set or removed.
                    clear_route_cache: true,
                    ..Default::default()
                }),
            })
        }
        Request::ResponseHeaders(_) => Response::ResponseHeaders(HeadersResponse::default()),
        Request::RequestBody(_) => Response::RequestBody(BodyResponse::default()),
        Request::ResponseBody(_) => Response::ResponseBody(BodyResponse::default()),
        Request::RequestTrailers(_) => Response::RequestTrailers(TrailersResponse::default()),
        Request::ResponseTrailers(_) => Response::ResponseTrailers(TrailersResponse::default()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use envoy_types::ext_authz::v3::pb::{Authorization, CheckRequest, HttpResponse};
    use envoy_types::pb::envoy::config::core::v3::{HeaderMap, HeaderValue, HeaderValueOption};
    use envoy_types::pb::envoy::service::auth::v3::AttributeContext;
    use envoy_types::pb::envoy::service::auth::v3::attribute_context::{HttpRequest, Request};
    use envoy_types::pb::envoy::service::ext_proc::v3::HttpHeaders;
    use envoy_types::pb::envoy::service::ext_proc::v3::processing_request::Request as Phase;
    use envoy_types::pb::envoy::service::ext_proc::v3::processing_response::Response;

    use super::{Callouts, continue_after};
    use crate::test_settings::protect;

    // Each header's name and value, from `value` or `raw_value`, whichever is set.
    fn read_set(options: &[HeaderValueOption]) -> Vec<(String, String)> {
        options
            .iter()
            .map(|option| {
                let header = option.header.as_ref().unwrap();
                let raw = String::from_utf8(header.raw_value.clone()).unwrap();
                (header.key.clone(), format!("{}{raw}", header.value))
            })
            .collect()
    }

    fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
        expected
            .iter()
            .map(|&(name, value)| (name.to_owned(), value.to_owned()))
            .collect()
    }

    async fn check(host: &str, protected: &str) -> (Vec<(String, String)>, Vec<String>) {
        let allow = Callouts {
            tickets: None,
            protected_hosts: Arc::new(protect(protected)),
        };
        let headers: HashMap<String, String> = [("x-rail-upstream", "plain"), ("accept", "*/*")]
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
            .collect();
        let request = CheckRequest {
            attributes: Some(AttributeContext {
                request: Some(Request {
                    http: Some(HttpRequest {
                        host: host.to_owned(),
                        headers,
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            }),
        };
        let response = allow
            .check(tonic::Request::new(request))
            .await
            .unwrap()
            .into_inner();
        let Some(HttpResponse::OkResponse(ok)) = response.http_response else {
            panic!("not allowed: {response:?}");
        };
        // ext_authz reads `value`.
        assert!(ok.headers.iter().all(|option| {
            option
                .header
                .as_ref()
                .is_some_and(|header| header.raw_value.is_empty())
        }));
        (read_set(&ok.headers), ok.headers_to_remove)
    }

    #[tokio::test]
    async fn ext_authz_routes_a_protected_host_from_its_host() {
        let (set, remove) = check("MCP.example.com:9443", "mcp.example.com").await;

        assert_eq!(
            set,
            pairs(&[
                ("x-rail-upstream", "tls"),
                (":authority", "mcp.example.com:9443")
            ])
        );
        assert_eq!(remove, ["x-rail", "x-rail-status"]);
    }

    #[tokio::test]
    async fn ext_authz_leaves_an_unprotected_host_unrouted() {
        let (set, remove) = check("open.example.com", "mcp.example.com").await;

        assert!(set.is_empty());
        assert_eq!(remove, ["x-rail", "x-rail-status", "x-rail-upstream"]);
    }

    fn process(authority: HeaderValue, protected: &str) -> (Vec<(String, String)>, Vec<String>) {
        let phase = Phase::RequestHeaders(HttpHeaders {
            headers: Some(HeaderMap {
                headers: vec![
                    authority,
                    HeaderValue {
                        key: "x-rail-upstream".to_owned(),
                        raw_value: b"tls".to_vec(),
                        ..Default::default()
                    },
                ],
            }),
            ..Default::default()
        });
        let Response::RequestHeaders(response) = continue_after(phase, None, &protect(protected))
        else {
            panic!("not a headers response");
        };
        let common = response.response.unwrap();
        assert!(common.clear_route_cache);
        let mutation = common.header_mutation.unwrap();
        // ext_proc reads `raw_value`.
        assert!(mutation.set_headers.iter().all(|option| {
            option
                .header
                .as_ref()
                .is_some_and(|header| header.value.is_empty())
        }));
        (read_set(&mutation.set_headers), mutation.remove_headers)
    }

    #[test]
    fn ext_proc_routes_a_protected_host_from_its_authority() {
        for authority in [
            HeaderValue {
                key: ":authority".to_owned(),
                raw_value: b"gw.internal".to_vec(),
                ..Default::default()
            },
            HeaderValue {
                key: ":authority".to_owned(),
                value: "gw.internal".to_owned(),
                ..Default::default()
            },
        ] {
            let (set, remove) = process(authority, "http://gw.internal:8080");

            assert_eq!(
                set,
                pairs(&[
                    ("x-rail-upstream", "plain"),
                    (":authority", "gw.internal:8080")
                ])
            );
            assert_eq!(remove, ["x-rail", "x-rail-status"]);
        }
    }

    #[test]
    fn ext_proc_removes_the_agents_route_to_an_unprotected_host() {
        let authority = HeaderValue {
            key: ":authority".to_owned(),
            raw_value: b"open.example.com".to_vec(),
            ..Default::default()
        };

        let (set, remove) = process(authority, "gw.internal");

        assert!(set.is_empty());
        assert_eq!(remove, ["x-rail", "x-rail-status", "x-rail-upstream"]);
    }
}
