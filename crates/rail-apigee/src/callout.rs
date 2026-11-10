//! Each request Apigee sends, judged, and answered with what Apigee should change.
//!
//! Apigee applies only what the returned maps contain, and writes each item of a header returned
//! as a header line of its own. So the answer removes the `x-rail` headers and sets our flow
//! variables, and touches nothing else.

use std::sync::Arc;

use apigee_callout_proto::apigee::external_callout_service_server::ExternalCalloutService;
use apigee_callout_proto::apigee::{FlowVariable, MessageContext, Strings, flow_variable};
use percent_encoding::percent_decode_str;
use rail_runtime::{Answer, Call, Gateway};

// The flow variables the Apigee proxy reads.
const DECISION: &str = "rail.decision";
const STATUS: &str = "rail.status";
const BODY: &str = "rail.body";
const ALLOW: &str = "allow";
const REFUSE: &str = "refuse";

const XRAIL: &str = "x-rail";

/// The ExternalCallout service; with no gateway, Rail Center is disabled and every call allowed.
#[derive(Clone)]
pub struct Callout {
    pub gateway: Option<Arc<Gateway>>,
}

#[tonic::async_trait]
impl ExternalCalloutService for Callout {
    async fn process_message(
        &self,
        request: tonic::Request<MessageContext>,
    ) -> Result<tonic::Response<MessageContext>, tonic::Status> {
        let mut context = request.into_inner();
        let answer = match &self.gateway {
            Some(gateway) => {
                let headers = read_headers(&context);
                let request = context.request.get_or_insert_default();
                let call = Call {
                    path: read_path(&request.uri),
                    method: request.verb.clone(),
                    // Moved in and back out: it can be 100 MiB.
                    body: std::mem::take(&mut request.content),
                    headers,
                };
                let (answer, call) = gateway.handle(call).await;
                context.request.get_or_insert_default().content = call.body;
                answer
            }
            None => Answer::Forward,
        };
        Ok(tonic::Response::new(build_answer(context, answer)))
    }
}

/// The path of `uri`, without the query, percent-decoded as Python's `unquote` does.
pub fn read_path(uri: &str) -> String {
    let path = uri.split_once('?').map_or(uri, |(path, _)| path);
    let path = percent_decode_str(path).decode_utf8_lossy();
    if path.is_empty() {
        "/".to_owned()
    } else {
        path.into_owned()
    }
}

// Every item as a header line: Apigee splits a value on commas and sends each part as an item.
fn read_headers(context: &MessageContext) -> Vec<(String, String)> {
    let Some(request) = &context.request else {
        return Vec::new();
    };
    request
        .headers
        .iter()
        .flat_map(|(name, values)| {
            values
                .strings
                .iter()
                .map(move |value| (name.clone(), value.clone()))
        })
        .collect()
}

/// The received context, with only our changes in its maps: Apigee's own fields are returned as
/// received, so none of them changes.
pub fn build_answer(mut context: MessageContext, answer: Answer) -> MessageContext {
    let request = context.request.get_or_insert_default();
    // An empty list removes the header. `x-rail` is always named: a header that isn't there is
    // left alone.
    let mut removed: Vec<String> = request
        .headers
        .keys()
        .filter(|name| is_x_rail_header(name))
        .cloned()
        .collect();
    removed.push(XRAIL.to_owned());
    request.headers = removed
        .into_iter()
        .map(|name| (name, Strings::default()))
        .collect();
    request.form_params.clear();
    request.query_params.clear();

    let flow = &mut context.additional_flow_variables;
    flow.clear();
    match answer.refusal() {
        None => {
            flow.insert(DECISION.to_owned(), text(ALLOW));
        }
        Some((status, body)) => {
            flow.insert(DECISION.to_owned(), text(REFUSE));
            flow.insert(STATUS.to_owned(), text(&status.to_string()));
            flow.insert(BODY.to_owned(), text(body));
        }
    }
    context
}

fn is_x_rail_header(name: &str) -> bool {
    let lowered = name.to_ascii_lowercase();
    lowered == XRAIL || lowered.starts_with("x-rail-")
}

fn text(value: &str) -> FlowVariable {
    FlowVariable {
        value: Some(flow_variable::Value::String(value.to_owned())),
    }
}
