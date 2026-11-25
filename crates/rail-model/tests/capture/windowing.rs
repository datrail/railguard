use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rail_model::body::{BodyRead, Unread};
use rail_model::capture::{
    BodyUnreadReason, CallOutcome, ClosedWindow, CompiledPlan, ObservedCall, Windowing,
    compile_plan, parse_plan,
};
use serde_json::Value;
use uuid::Uuid;

const VECTORS: &str = include_str!("../../../../vectors/capture_window_conformance_cases.json");

// What the capture task tells the windowing, in order: the plan held (by index, `None` for none)
// at seconds after the epoch, a call received then, or calls lost before the queue.
enum Event {
    Hold(Option<usize>, u64),
    Record(Option<usize>, u64),
    Dropped(u64),
}

use Event::{Dropped, Hold, Record};

fn time(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

// Plans told apart by their version, each with 60 s windows.
fn plans() -> [Arc<CompiledPlan>; 2] {
    let vectors: Value = serde_json::from_str(VECTORS).unwrap();
    [1, 2].map(|version| {
        let mut plan = vectors["plan"].clone();
        plan["window_seconds"] = 60.into();
        plan["plan_version"] = version.into();
        Arc::new(compile_plan(parse_plan(&plan.to_string()).unwrap()))
    })
}

fn call(method: &str, path: &str, why: Unread) -> ObservedCall {
    ObservedCall {
        authority: b"api.github.com".to_vec(),
        method: method.as_bytes().to_vec(),
        path: path.as_bytes().to_vec(),
        body: BodyRead::Unread(why),
        body_bytes: 0,
    }
}

// `v<plan_version> <start>-<end>, <calls> calls, <dropped> dropped`, times as minutes and seconds.
fn describe(closed: ClosedWindow) -> String {
    let document = closed.into_document(Uuid::nil(), time(0), 0);
    let written = serde_json::to_value(&document).unwrap();
    let at = |key: &str| {
        let text = written["window"][key].as_str().unwrap();
        text["1970-01-01T00:".len()..text.len() - 1].to_owned()
    };
    format!(
        "v{} {}-{}, {} calls, {} dropped",
        written["plan_version"],
        at("start"),
        at("end"),
        written["sequence"].as_array().unwrap().len(),
        written["truncated"]["calls_dropped"],
    )
}

// Each window closed, described, and each call dropped.
fn run(events: &[Event]) -> Vec<String> {
    let plans = plans();
    let held = |plan: Option<usize>| plan.map(|plan| Arc::clone(&plans[plan]));
    let mut windowing = Windowing::default();
    let mut said = Vec::new();
    for event in events {
        match *event {
            Hold(plan, at) => said.extend(windowing.hold(held(plan), time(at)).map(describe)),
            Record(plan, at) => {
                let get = call("GET", "/a", Unread::NotSent);
                let recorded = windowing.record(held(plan), &get, time(at));
                said.extend(recorded.closed.map(describe));
                if matches!(recorded.outcome, CallOutcome::Dropped) {
                    said.push("call dropped".to_owned());
                }
            }
            Dropped(calls) => windowing.count_dropped(calls),
        }
    }
    said
}

#[test]
fn test_windows_end_on_a_boundary_and_start_when_opened() {
    let cases: [(&str, &[Event], &[&str]); 6] = [
        (
            "the first window from the plan applied; a plan change past the boundary opens no window for the old plan",
            &[Hold(Some(0), 20), Hold(Some(1), 61), Hold(None, 70)],
            &[
                "v1 00:20-01:00, 0 calls, 0 dropped",
                "v2 01:01-01:10, 0 calls, 0 dropped",
            ],
        ),
        (
            "a call past the end closes the window first",
            &[Hold(Some(0), 0), Record(Some(0), 60), Hold(None, 70)],
            &[
                "v1 00:00-01:00, 0 calls, 0 dropped",
                "v1 01:00-01:10, 1 calls, 0 dropped",
            ],
        ),
        (
            "a plan change cuts the window and the same plan doesn't",
            &[
                Hold(Some(0), 0),
                Hold(Some(0), 30),
                Hold(Some(1), 40),
                Hold(None, 50),
            ],
            &[
                "v1 00:00-00:40, 0 calls, 0 dropped",
                "v2 00:40-00:50, 0 calls, 0 dropped",
            ],
        ),
        (
            "past its end, a window opens the next when seen; a clock stepped back before its start closes it at its last call",
            &[
                Hold(Some(0), 0),
                Hold(Some(0), 61),
                Record(Some(0), 62),
                Hold(Some(0), 30),
                Hold(None, 40),
            ],
            &[
                "v1 00:00-01:00, 0 calls, 0 dropped",
                "v1 01:01-01:02, 1 calls, 0 dropped",
                "v1 00:30-00:40, 0 calls, 0 dropped",
            ],
        ),
        (
            "after a smaller step back the next window starts at the last call",
            &[
                Hold(Some(0), 0),
                Record(Some(0), 50),
                Hold(Some(1), 40),
                Hold(None, 55),
            ],
            &[
                "v1 00:00-00:50, 1 calls, 0 dropped",
                "v2 00:50-00:55, 0 calls, 0 dropped",
            ],
        ),
        (
            "a call after the plan went closes the window and is dropped; drops with none are no window's",
            &[
                Hold(Some(0), 0),
                Dropped(2),
                Record(None, 30),
                Dropped(3),
                Hold(Some(0), 130),
                Hold(None, 140),
            ],
            &[
                "v1 00:00-00:30, 0 calls, 2 dropped",
                "call dropped",
                "v1 02:10-02:20, 0 calls, 0 dropped",
            ],
        ),
    ];
    for (name, events, expected) in cases {
        assert_eq!(run(events), expected, "{name}");
    }
}

#[test]
fn test_tells_when_the_open_window_ends_and_why_a_body_went_unread() {
    let [plan, _] = plans();
    let mut windowing = Windowing::default();
    assert_eq!(windowing.end(), None);
    assert!(windowing.hold(Some(Arc::clone(&plan)), time(60)).is_none());
    assert_eq!(windowing.end(), Some(time(120)));
    let past_cap = call("POST", "/mcp", Unread::PastCap);
    let recorded = windowing.record(Some(plan), &past_cap, time(70));
    assert!(matches!(
        recorded.outcome,
        CallOutcome::Counted {
            body_unread: Some(BodyUnreadReason::PastCap)
        }
    ));
}
