use std::sync::Arc;

use rail_model::body::{BodyBuffer, BodyRead, ByteBudget};

mod authority;
mod body;
mod capture;
mod condition;
mod policy;
mod redaction;
mod request_line;
mod sanitize;
mod timestamp;

// A body kept whole, within no cap or budget.
fn keep_body(body: &[u8]) -> BodyRead {
    let mut buffer = BodyBuffer::new(usize::MAX, &ByteBudget::new(usize::MAX));
    buffer.append(body);
    BodyRead::Whole(Arc::new(buffer.finish().unwrap()))
}
