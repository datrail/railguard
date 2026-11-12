//! Each request Apigee sends, judged, and answered with what Apigee should change.
//!
//! Apigee applies only what the returned maps contain, and writes each item of a header returned
//! as a header line of its own. So the answer removes the `x-rail` headers and any name with `_`,
//! sets our flow variables, and touches nothing else.

use std::sync::Arc;

use apigee_callout_proto::apigee::external_callout_service_server::ExternalCalloutService;
use apigee_callout_proto::apigee::{FlowVariable, MessageContext, Strings, flow_variable};
use prost::Message;
use rail_runtime::{Answer, Call, Gateway, XRAIL, is_withheld_header, read_path};

// The flow variables the Apigee proxy reads.
const DECISION: &str = "rail.decision";
const STATUS: &str = "rail.status";
const BODY: &str = "rail.body";
const ALLOW: &str = "allow";
const REFUSE: &str = "refuse";

// The answer echoes the content, plus our flow variables and removals: a few hundred bytes.
pub(crate) const SEND_HEADROOM_BYTES: usize = 1024;
// Apigee takes a callout's answer of at most 4 MiB.
const MAX_APIGEE_ANSWER_BYTES: usize = 4 * 1024 * 1024;
/// The largest request answered as judged: its answer, carrying the content back, still fits in
/// what Apigee takes. A larger one is refused unjudged, as Apigee would fail its answer.
pub const MAX_ANSWERED_BYTES: usize = MAX_APIGEE_ANSWER_BYTES - SEND_HEADROOM_BYTES;

/// The ExternalCallout service; with no gateway, Rail Center is disabled and every call it can
/// answer allowed.
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
        let bytes = context.encoded_len();
        if bytes > MAX_ANSWERED_BYTES {
            tracing::warn!(
                message_id = context.message_id,
                bytes,
                limit = MAX_ANSWERED_BYTES,
                "refused unjudged: Apigee takes an answer of at most 4 MiB, and it carries the request's content back"
            );
            // Nothing is forwarded, so the content needn't go back.
            context.request.get_or_insert_default().content.clear();
            return Ok(tonic::Response::new(build_answer(
                context,
                Answer::TooLarge,
            )));
        }
        let answer = match &self.gateway {
            Some(gateway) => {
                let headers = read_headers(&context);
                let request = context.request.get_or_insert_default();
                let call = Call {
                    path: read_path(&request.uri),
                    method: request.verb.clone(),
                    // Moved in and back out: it can be up to 4 MiB.
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
        .filter(|name| is_withheld_header(&name.to_ascii_lowercase()))
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

fn text(value: &str) -> FlowVariable {
    FlowVariable {
        value: Some(flow_variable::Value::String(value.to_owned())),
    }
}
