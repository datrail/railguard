use std::sync::Arc;

use rail_model::body::{BodyBuffer, BodyRead, ByteBudget};
use rail_model::request::ObservedRequest;

mod authority;
mod body;
mod capture;
mod condition;
mod policy;
mod request;
mod request_line;
mod sanitize;
mod vectors;

// A test body's budget: past any test's, but finite, so a parser that never stops runs out.
const BYTE_BUDGET_BYTES: usize = 32 << 20;

// A body kept whole, within no cap.
fn keep_body(body: &[u8]) -> BodyRead {
    let mut buffer = BodyBuffer::new(usize::MAX, &ByteBudget::new(BYTE_BUDGET_BYTES));
    buffer.append(body);
    BodyRead::Whole(Arc::new(buffer.finish().unwrap()))
}

// A POST to `/mcp` sending `lines` and `body`, adopted on `budget`, kept outside it if past it.
fn request_on(budget: &Arc<ByteBudget>, body: &[u8], lines: &[(&str, &str)]) -> ObservedRequest {
    let mut buffer = BodyBuffer::new(usize::MAX, budget);
    buffer.keep_past_budget();
    buffer.adopt(body.to_vec());
    let read = BodyRead::Whole(Arc::new(buffer.finish().unwrap()));
    let lines = lines
        .iter()
        .map(|(name, value)| (name.as_bytes().to_vec(), value.as_bytes().to_vec()))
        .collect();
    let (method, path) = (b"POST".to_vec(), b"/mcp".to_vec());
    ObservedRequest::new(Vec::new(), method, path, lines, read, body.len() as u64)
}

// A request with no header lines, as the proxy hands one to capture.
fn observed(
    authority: &str,
    method: &str,
    path: &str,
    body: BodyRead,
    bytes: u64,
) -> ObservedRequest {
    let [authority, method, path] = [authority, method, path].map(|part| part.as_bytes().to_vec());
    ObservedRequest::new(authority, method, path, Vec::new(), body, bytes)
}
