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

// What happens, in order, at seconds after ten.
enum Event {
    Hold(Option<usize>, f64),
    Advance(f64),
    Record(f64),
    Dropped(u64),
    Close(f64),
}

use Event::{Advance, Close, Dropped, Hold, Record};

fn time(seconds: f64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(TEN) + Duration::from_secs_f64(seconds)
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
    let document = closed.into_document(Uuid::nil(), time(0.0), 0);
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

fn run(events: &[Event]) -> (Vec<String>, Vec<bool>) {
    let plans = plans();
    let mut windowing = Windowing::new();
    let mut closed = Vec::new();
    let mut counted = Vec::new();
    for event in events {
        let window = match *event {
            Hold(plan, at) => windowing.hold(plan.map(|plan| Arc::clone(&plans[plan])), time(at)),
            Advance(at) => windowing.advance(time(at)),
            Record(at) => {
                let recorded = windowing.record(&call(), time(at));
                counted.push(recorded.is_some());
                recorded.and_then(|recorded| recorded.closed)
            }
            Dropped(calls) => {
                windowing.count_dropped(calls);
                None
            }
            Close(at) => {
                closed.extend(std::mem::take(&mut windowing).close(time(at)).map(describe));
                None
            }
        };
        closed.extend(window.map(describe));
    }
    (closed, counted)
}

#[test]
fn test_windows_end_on_a_boundary_and_start_when_opened() {
    let cases: [(&str, &[Event], &[&str]); 8] = [
        (
            "the first window from the plan applied, the next from the boundary seen",
            &[
                Hold(Some(0), 20.0),
                Record(30.0),
                Advance(60.003),
                Record(61.0),
            ],
            &["v1 00:20-01:00, 1 calls, 0 dropped"],
        ),
        (
            "a late boundary isn't stretched back to",
            &[Hold(Some(0), 0.0), Advance(60.003), Advance(120.0)],
            &[
                "v1 00:00-01:00, 0 calls, 0 dropped",
                "v1 01:00.003-02:00, 0 calls, 0 dropped",
            ],
        ),
        (
            "a time not seen is no window's",
            &[
                Hold(Some(0), 0.0),
                Record(30.0),
                Advance(1810.0),
                Close(1830.0),
            ],
            &[
                "v1 00:00-01:00, 1 calls, 0 dropped",
                "v1 30:10-30:30, 0 calls, 0 dropped",
            ],
        ),
        (
            "a call past the end closes the window first",
            &[Hold(Some(0), 0.0), Record(60.0), Close(70.0)],
            &[
                "v1 00:00-01:00, 0 calls, 0 dropped",
                "v1 01:00-01:10, 1 calls, 0 dropped",
            ],
        ),
        (
            "a plan change cuts the window; the same plan doesn't",
            &[
                Hold(Some(0), 0.0),
                Hold(Some(0), 30.0),
                Hold(Some(1), 40.0),
                Advance(60.0),
            ],
            &[
                "v1 00:00-00:40, 0 calls, 0 dropped",
                "v2 00:40-01:00, 0 calls, 0 dropped",
            ],
        ),
        (
            "a plan change past the boundary opens no window for the old plan",
            &[Hold(Some(0), 0.0), Hold(Some(1), 60.2), Close(70.0)],
            &[
                "v1 00:00-01:00, 0 calls, 0 dropped",
                "v2 01:00.2-01:10, 0 calls, 0 dropped",
            ],
        ),
        (
            "a clock stepped back opens no window over the one closed",
            &[
                Hold(Some(0), 0.0),
                Record(50.0),
                Hold(Some(1), 30.0),
                Close(55.0),
            ],
            &[
                "v1 00:00-00:50, 1 calls, 0 dropped",
                "v2 00:50-00:55, 0 calls, 0 dropped",
            ],
        ),
        (
            "no plan closes the window, drops are no window's, and none opens until one is held",
            &[
                Hold(Some(0), 0.0),
                Dropped(2),
                Hold(None, 30.0),
                Dropped(3),
                Advance(120.0),
                Hold(Some(0), 130.0),
                Close(140.0),
            ],
            &[
                "v1 00:00-00:30, 0 calls, 2 dropped",
                "v1 02:10-02:20, 0 calls, 0 dropped",
            ],
        ),
    ];
    for (name, events, expected) in cases {
        assert_eq!(run(events).0, expected, "{name}");
    }
}

#[test]
fn test_a_call_with_no_plan_held_is_dropped() {
    let events = [
        Record(0.0),
        Hold(Some(0), 10.0),
        Record(20.0),
        Hold(None, 30.0),
        Record(40.0),
    ];
    assert_eq!(run(&events).1, [false, true, false]);
}

#[test]
fn test_tells_when_the_open_window_ends() {
    let [plan, _] = plans();
    let mut windowing = Windowing::new();
    assert_eq!(windowing.end(), None);
    // On a boundary, the window runs to the next.
    windowing.hold(Some(plan), time(60.0));
    assert_eq!(windowing.end(), Some(time(120.0)));
    windowing.hold(None, time(70.0));
    assert_eq!(windowing.end(), None);
}
