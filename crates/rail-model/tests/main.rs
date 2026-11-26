use std::sync::Arc;

use rail_model::body::{BodyBuffer, BodyRead, ByteBudget};

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
