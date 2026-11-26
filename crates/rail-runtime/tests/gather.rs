use rail_model::body::{BodyRead, ByteBudget, Unread};
use rail_runtime::{Ending, RequestGatherer};

const CAP_BYTES: usize = 4;

// A request declaring `declared`, if any, sent `sent`, if any, as its host saw it end.
fn gather(
    declared: Option<&str>,
    sent: Option<&[u8]>,
    ending: Ending,
    budget_bytes: usize,
) -> (Result<Vec<u8>, Unread>, u64) {
    let length = declared.map(|length| ("Content-Length", length));
    let lines = [(":method", "POST"), (":path", "/mcp")]
        .into_iter()
        .chain(length);
    let mut gatherer = RequestGatherer::start(lines, CAP_BYTES, &ByteBudget::new(budget_bytes));
    if let Some(sent) = sent {
        gatherer.adopt(sent.to_vec());
    }
    let request = gatherer.finish(ending);
    let body = match request.body() {
        BodyRead::Whole(body) => Ok(body.as_bytes().to_vec()),
        BodyRead::Unread(why) => Err(*why),
    };
    (body, request.body_bytes())
}

#[test]
fn a_body_is_read_whole_only_if_it_came_whole_within_the_cap_and_the_budget() {
    use Ending::{MayBeCut, NotSent, Whole};
    let whole = |bytes: &[u8]| Ok(bytes.to_vec());
    let roomy = usize::MAX;
    let cases = [
        (None, Some(&b"ab"[..]), Whole, roomy, whole(b"ab"), 2),
        // A declared length below what was sent isn't the body's.
        (Some("1"), Some(b"ab"), Whole, roomy, whole(b"ab"), 2),
        (Some("3"), None, NotSent, roomy, Err(Unread::NotSent), 3),
        // Declared empty, a body not sent is whole.
        (Some("0"), None, NotSent, roomy, whole(b""), 0),
        (None, Some(b"abcde"), Whole, roomy, Err(Unread::PastCap), 5),
        (
            Some("5"),
            Some(b"ab"),
            MayBeCut,
            roomy,
            Err(Unread::PastCap),
            5,
        ),
        (None, Some(b"ab"), MayBeCut, roomy, Err(Unread::CutShort), 2),
        (Some("2"), Some(b"ab"), MayBeCut, roomy, whole(b"ab"), 2),
        (
            Some("3"),
            Some(b"ab"),
            Whole,
            roomy,
            Err(Unread::CutShort),
            3,
        ),
        (Some("3"), None, Whole, roomy, Err(Unread::NotSent), 3),
        (None, Some(b"ab"), Whole, 1, Err(Unread::PastBudget), 2),
    ];
    for (declared, sent, ending, budget_bytes, body, body_bytes) in cases {
        assert_eq!(
            gather(declared, sent, ending, budget_bytes),
            (body, body_bytes),
            "{declared:?} {sent:?} {ending:?} {budget_bytes}"
        );
    }
}
