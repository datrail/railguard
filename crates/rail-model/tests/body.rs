mod mcp;
mod tree;

use rail_model::body::mcp::McpReading;
use rail_model::body::{BodyBuffer, ByteBudget, RequestBody, Unkept, Unparsed};

// A body kept whole, with no cap or budget to meet.
fn keep(body: &[u8]) -> RequestBody {
    let mut buffer = BodyBuffer::new(usize::MAX, &ByteBudget::new(usize::MAX));
    buffer.append(body);
    buffer.finish().unwrap()
}

#[test]
fn test_keeps_a_body_up_to_its_cap_counting_every_byte() {
    let budget = ByteBudget::new(usize::MAX);
    let mut whole = BodyBuffer::new(5, &budget);
    whole.append(b"abc");
    whole.append(b"de");
    assert_eq!(whole.received_bytes(), 5);
    assert_eq!(whole.finish().unwrap().as_bytes(), b"abcde");

    let mut past = BodyBuffer::new(5, &budget);
    for chunk in [&b"abc"[..], b"def", b"g"] {
        past.append(chunk);
    }
    assert_eq!(past.received_bytes(), 7);
    assert_eq!(past.finish().unwrap_err(), Unkept::PastCap);
}

#[test]
fn test_shares_one_budget_between_bodies_until_each_is_dropped() {
    let budget = ByteBudget::new(10);
    let kept = |bytes: usize| {
        let mut buffer = BodyBuffer::new(usize::MAX, &budget);
        buffer.append(&vec![b'0'; bytes]);
        buffer.finish()
    };
    let first = kept(8).unwrap();
    assert_eq!(kept(4).unwrap_err(), Unkept::PastBudget);
    drop(first);
    assert!(kept(4).is_ok());

    // A body refused gives its bytes back at once, not when it's dropped.
    let mut refused = BodyBuffer::new(usize::MAX, &budget);
    refused.append(&[b'0'; 8]);
    refused.append(&[b'0'; 8]);
    assert!(kept(10).is_ok());
    assert_eq!(refused.finish().unwrap_err(), Unkept::PastBudget);
}

#[test]
fn test_charges_a_bodys_tree_to_the_budget_too() {
    let body = b"[0, 0, 0]";
    let kept = |budget: &std::sync::Arc<ByteBudget>, bytes: &[u8]| {
        let mut buffer = BodyBuffer::new(usize::MAX, budget);
        buffer.append(bytes);
        buffer.finish()
    };
    let tight = ByteBudget::new(body.len());
    let unparsed = kept(&tight, body).unwrap();
    assert_eq!(unparsed.root().err(), Some(Unparsed::PastBudget));
    assert_eq!(unparsed.mcp(), &McpReading::Unreadable);

    let roomy = ByteBudget::new(1 << 20);
    let parsed = kept(&roomy, body).unwrap();
    assert!(parsed.root().is_ok());
    let left = (1 << 20) - body.len();
    assert_eq!(
        kept(&roomy, &vec![b'0'; left]).unwrap_err(),
        Unkept::PastBudget
    );
    drop(parsed);
    assert!(kept(&roomy, &vec![b'0'; left]).is_ok());
}
