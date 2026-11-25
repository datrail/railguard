use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rail_model::body::{BodyRead, Unread};
use rail_model::capture::{
    ClosedWindow, CompiledPlan, ObservedCall, Windowing, compile_plan, parse_plan,
};
use serde_json::Value;
use uuid::Uuid;

const VECTORS: &str = include_str!("../../../../vectors/capture_window_conformance_cases.json");

// 2026-11-10T10:00:00Z: the times below are seconds after it.
const TEN: u64 = 1_794_304_800;

// What the capture task tells the windowing, in order: the plan held (by index, `None` for none)
// at seconds after ten, a call received then, or calls lost before the queue.
enum Event {
    Hold(Option<usize>, u64),
    Record(Option<usize>, u64),
    Dropped(u64),
}

use Event::{Dropped, Hold, Record};

fn time(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(TEN + seconds)
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

fn call() -> ObservedCall {
    ObservedCall {
        authority: b"api.github.com".to_vec(),
        method: b"GET".to_vec(),
        path: b"/a".to_vec(),
        body: BodyRead::Unread(Unread::NotSent),
        body_bytes: 0,
    }
}

// `v<plan_version> <start>-<end>, <calls> calls, <dropped> dropped`, times as minutes and seconds
// after ten.
fn describe(closed: ClosedWindow) -> String {
    let document = closed.into_document(Uuid::nil(), time(0), 0);
    let written = serde_json::to_value(&document).unwrap();
    let at = |key: &str| {
        let text = written["window"][key].as_str().unwrap();
        text["2026-11-10T10:".len()..text.len() - 1].to_owned()
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
        let closed = match *event {
            Hold(plan, at) => windowing.hold(held(plan), time(at)),
            Record(plan, at) => match windowing.record(held(plan), &call(), time(at)) {
                Some(counted) => counted.closed,
                None => {
                    said.push("call dropped".to_owned());
                    None
                }
            },
            Dropped(calls) => {
                windowing.count_dropped(calls);
                None
            }
        };
        said.extend(closed.map(describe));
    }
    said
}

#[test]
fn test_windows_end_on_a_boundary_and_start_when_opened() {
    let cases: [(&str, &[Event], &[&str]); 7] = [
        (
            "the first window from the plan applied; past a time not seen, the next from then",
            &[
                Hold(Some(0), 20),
                Record(Some(0), 30),
                Hold(Some(0), 1810),
                Hold(None, 1830),
            ],
            &[
                "v1 00:20-01:00, 1 calls, 0 dropped",
                "v1 30:10-30:30, 0 calls, 0 dropped",
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
            "a plan change cuts the window, the same plan doesn't, and none past the end closes there",
            &[
                Hold(Some(0), 0),
                Hold(Some(0), 30),
                Hold(Some(1), 40),
                Hold(None, 70),
            ],
            &[
                "v1 00:00-00:40, 0 calls, 0 dropped",
                "v2 00:40-01:00, 0 calls, 0 dropped",
            ],
        ),
        (
            "a plan change past the boundary opens no window for the old plan",
            &[Hold(Some(0), 0), Hold(Some(1), 61), Hold(None, 70)],
            &[
                "v1 00:00-01:00, 0 calls, 0 dropped",
                "v2 01:01-01:10, 0 calls, 0 dropped",
            ],
        ),
        (
            "a call under another plan changes it first",
            &[Hold(Some(0), 0), Record(Some(1), 30), Hold(None, 40)],
            &[
                "v1 00:00-00:30, 0 calls, 0 dropped",
                "v2 00:30-00:40, 1 calls, 0 dropped",
            ],
        ),
        (
            "a clock stepped back before the window's start closes it at its last call",
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
            "no plan closes the window, drops with none are no window's, and none opens until one is held",
            &[
                Hold(Some(0), 0),
                Dropped(2),
                Hold(None, 30),
                Dropped(3),
                Record(None, 40),
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
fn test_tells_when_the_open_window_ends() {
    let [plan, _] = plans();
    let mut windowing = Windowing::default();
    assert_eq!(windowing.end(), None);
    windowing.hold(Some(plan), time(60));
    assert_eq!(windowing.end(), Some(time(120)));
}
