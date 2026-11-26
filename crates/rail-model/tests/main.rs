use std::sync::Arc;

use rail_model::body::{BodyBuffer, BodyRead, ByteBudget};

mod authority;
mod body;
mod capture;
mod condition;
mod policy;
mod request_line;
mod sanitize;
mod vectors;

// A body kept whole, within no cap; a budget only so a parser that never stops runs out.
fn keep_body(body: &[u8]) -> BodyRead {
    let mut buffer = BodyBuffer::new(usize::MAX, &ByteBudget::new(32 << 20));
    buffer.append(body);
    BodyRead::Whole(Arc::new(buffer.finish().unwrap()))
}
