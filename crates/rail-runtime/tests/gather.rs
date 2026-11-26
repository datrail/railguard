use rail_model::body::{BodyRead, ByteBudget, Unread};
use rail_runtime::{BodyEnding, Lines, RequestGatherer};

const CAP_BYTES: usize = 4;

// A request declaring each of `declared` as a `Content-Length`, sent `sent`, if any, as its host
// saw it end.
fn gather(
    declared: &[&str],
    sent: Option<&[u8]>,
    ending: BodyEnding,
    budget_bytes: usize,
) -> (Result<Vec<u8>, Unread>, u64) {
    let lengths = declared.iter().map(|length| ("Content-Length", *length));
    let lines = [(":method", "POST"), (":path", "/mcp")]
        .into_iter()
        .chain(lengths)
        .chain([("content-type", "application/json")]);
    let mut gatherer = RequestGatherer::start(
        lines,
        Lines::None,
        CAP_BYTES,
        &ByteBudget::new(budget_bytes),
    );
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
    use BodyEnding::{CutShort, MayBeCut, NotSent, Whole};
    let whole = |bytes: &[u8]| Ok(bytes.to_vec());
    let roomy = usize::MAX;
    let cases = [
        (&[][..], Some(&b"abcd"[..]), Whole, roomy, whole(b"abcd"), 4),
        // A declared length below what was sent isn't the body's.
        (&["1"], Some(b"ab"), Whole, roomy, whole(b"ab"), 2),
        // Not sent, whatever its length.
        (&["5"], None, NotSent, roomy, Err(Unread::NotSent), 5),
        // Declared empty, a body not sent is whole.
        (&["0"], None, NotSent, roomy, whole(b""), 0),
        (
            &[],
            Some(b"abcde"),
            MayBeCut,
            roomy,
            Err(Unread::PastCap),
            5,
        ),
        (
            &["5"],
            Some(b"ab"),
            MayBeCut,
            roomy,
            Err(Unread::PastCap),
            5,
        ),
        (&[], Some(b"ab"), MayBeCut, roomy, Err(Unread::CutShort), 2),
        (&["2"], Some(b"ab"), MayBeCut, roomy, whole(b"ab"), 2),
        // The first `Content-Length` is the body's.
        (&["2", "3"], Some(b"ab"), Whole, roomy, whole(b"ab"), 2),
        (&["3"], Some(b"ab"), Whole, roomy, Err(Unread::CutShort), 3),
        (&["3"], None, Whole, roomy, Err(Unread::NotSent), 3),
        (&["3"], None, CutShort, roomy, Err(Unread::CutShort), 3),
        (&[], Some(b"ab"), Whole, 1, Err(Unread::PastBudget), 2),
    ];
    for (declared, sent, ending, budget_bytes, body, body_bytes) in cases {
        assert_eq!(
            gather(declared, sent, ending, budget_bytes),
            (body, body_bytes),
            "{declared:?} {sent:?} {ending:?} {budget_bytes}"
        );
    }
}

// The gateway's request keeps its header lines, but not the pseudo-headers; capture's keeps none.
#[test]
fn a_request_keeps_its_header_lines_only_if_asked_to() {
    let lines = [
        (":method", "POST"),
        (":scheme", "https"),
        ("x-a", "1"),
        ("X-B", "2"),
    ];
    for (keep, kept) in [
        (Lines::All, vec![("x-a", "1"), ("X-B", "2")]),
        (Lines::None, vec![]),
    ] {
        let gatherer = RequestGatherer::start(lines, keep, CAP_BYTES, &ByteBudget::new(usize::MAX));
        let request = gatherer.finish(BodyEnding::Whole);
        assert_eq!(request.header_lines().collect::<Vec<_>>(), kept, "{keep:?}");
    }
}
