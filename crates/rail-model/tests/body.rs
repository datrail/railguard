mod json;
mod tree;

use std::sync::Arc;

use rail_model::body::{BodyBuffer, ByteBudget, RequestBody, Unkept, Unread};

// A body kept whole, within no cap.
fn keep(body: &[u8]) -> RequestBody {
    keep_within(&ByteBudget::new(crate::BYTE_BUDGET_BYTES), body).unwrap()
}

fn keep_within(budget: &Arc<ByteBudget>, body: &[u8]) -> Result<RequestBody, Unkept> {
    let mut buffer = BodyBuffer::new(usize::MAX, budget);
    buffer.append(body);
    buffer.finish()
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

// Without a copy, its capacity charged; after a chunk, appended.
#[test]
fn test_adopts_a_body_as_appending_it_would() {
    let budget = ByteBudget::new(8);
    let mut first = BodyBuffer::new(5, &budget);
    first.adopt(b"abc".to_vec());
    first.adopt(b"de".to_vec());
    assert_eq!(first.finish().unwrap().as_bytes(), b"abcde");

    let mut past = BodyBuffer::new(5, &budget);
    past.adopt(b"abcdef".to_vec());
    assert_eq!(past.received_bytes(), 6);
    assert_eq!(past.finish().unwrap_err(), Unkept::PastCap);

    let mut roomy = BodyBuffer::new(5, &budget);
    roomy.adopt(Vec::with_capacity(9));
    assert_eq!(roomy.finish().unwrap_err(), Unkept::PastBudget);
}

#[test]
fn test_shares_one_budget_between_bodies_until_each_is_dropped() {
    let budget = ByteBudget::new(16);
    let kept = |bytes: usize| keep_within(&budget, &vec![b'0'; bytes]);
    // In chunks that fill the budget exactly, an empty one last.
    let mut first = BodyBuffer::new(usize::MAX, &budget);
    for chunk in [&[b'0'; 4][..], &[b'0'; 4], &[b'0'; 8], &[]] {
        first.append(chunk);
    }
    let first = first.finish().unwrap();
    assert_eq!(kept(1).unwrap_err(), Unkept::PastBudget);
    drop(first);
    assert!(kept(16).is_ok());

    // A body refused gives its bytes back at once, and only once.
    let mut refused = BodyBuffer::new(usize::MAX, &budget);
    refused.append(&[b'0'; 8]);
    refused.append(&[b'0'; 16]);
    assert!(kept(16).is_ok());
    assert_eq!(refused.finish().unwrap_err(), Unkept::PastBudget);
    assert!(kept(16).is_ok());
    assert_eq!(kept(17).unwrap_err(), Unkept::PastBudget);
}

#[test]
fn test_charges_a_bodys_tree_to_the_budget_too() {
    let body = b"[0, 0, 0]";
    let roomy = ByteBudget::new(1 << 20);
    let parsed = keep_within(&roomy, body).unwrap();
    assert!(parsed.json().is_ok());
    let left = vec![b'0'; (1 << 20) - body.len()];
    assert_eq!(keep_within(&roomy, &left).unwrap_err(), Unkept::PastBudget);
    drop(parsed);
    assert!(keep_within(&roomy, &left).is_ok());
}

// The bytes can hold credentials.
#[test]
fn test_debugs_a_body_without_its_bytes() {
    let printed = format!("{:?}", keep(b"secret"));
    assert!(!printed.contains("secret"), "{printed}");
}

#[test]
fn test_reads_a_body_not_kept_as_unread_for_the_same_reason() {
    assert_eq!(Unread::from(Unkept::PastCap), Unread::PastCap);
    assert_eq!(Unread::from(Unkept::PastBudget), Unread::PastBudget);
}
