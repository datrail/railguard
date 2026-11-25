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

fn get() -> ObservedCall {
    call("GET", "/a", Unread::NotSent)
}

fn write(closed: ClosedWindow) -> Value {
    serde_json::to_value(closed.into_document(Uuid::nil(), time(0), 0)).unwrap()
}

// `v<plan_version> <start>-<end>, <calls> calls, <dropped> dropped`, times as minutes and seconds.
fn describe(closed: ClosedWindow) -> String {
    let written = write(closed);
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
                let recorded = windowing.record(held(plan), &get(), time(at));
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
            "the first window from the plan applied, the same plan keeping it; a plan change past the boundary opens no window for the old plan",
            &[
                Hold(Some(0), 20),
                Hold(Some(0), 30),
                Hold(Some(1), 61),
                Hold(None, 70),
            ],
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
            "a clock stepped back reads as the latest time written, across a plan change",
            &[
                Hold(Some(0), 0),
                Record(Some(0), 50),
                Hold(Some(1), 40),
                Record(Some(1), 45),
                Hold(None, 55),
            ],
            &[
                "v1 00:00-00:50, 1 calls, 0 dropped",
                "v2 00:50-00:55, 1 calls, 0 dropped",
            ],
        ),
        (
            "and across the plan gone",
            &[
                Hold(Some(0), 0),
                Record(Some(0), 50),
                Record(None, 40),
                Record(Some(0), 45),
                Hold(None, 55),
            ],
            &[
                "v1 00:00-00:50, 1 calls, 0 dropped",
                "call dropped",
                "v1 00:50-00:55, 1 calls, 0 dropped",
            ],
        ),
        (
            "a minute back is absorbed; further back, the window closes at its last call and the next opens then",
            &[
                Hold(Some(0), 100),
                Record(Some(0), 110),
                Hold(Some(1), 50),
                Hold(Some(1), 48),
                Hold(None, 55),
            ],
            &[
                "v1 01:40-01:50, 1 calls, 0 dropped",
                "v2 01:50-01:50, 0 calls, 0 dropped",
                "v2 00:48-00:55, 0 calls, 0 dropped",
            ],
        ),
        (
            "the plan gone closes the window; drops with none open are no window's",
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

// A fixed-seed generator: the same sequences every run.
struct Random(u64);

impl Random {
    fn below(&mut self, bound: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % bound
    }
}

// Seconds since the epoch of a time in January 1970, as a document writes one.
fn read_time(value: &Value) -> u64 {
    let text = value.as_str().unwrap();
    let field = |at: usize| text[at..at + 2].parse::<u64>().unwrap();
    (field(8) - 1) * 86_400 + field(11) * 3_600 + field(14) * 60 + field(17)
}

#[test]
fn test_windows_hold_every_call_in_order_within_their_span_whatever_the_clock_does() {
    let plans = plans();
    // Clocks stepping back up to a minute behind the latest time seen, then up to two hours: each
    // window's calls in order within its span, no longer than a window, overlapping only after a
    // step back past a minute, and each call counted stamped at its clock, or up to a minute later.
    for (seed, furthest_back) in (0..200).map(|seed| (seed, if seed % 2 == 0 { 60 } else { 7_200 }))
    {
        let mut random = Random(seed);
        let mut windowing = Windowing::default();
        let (mut clock, mut latest_seen) = (200_000, 200_000);
        // The clock each call counted was taken at, in order.
        let (mut closed, mut taken) = (Vec::new(), Vec::new());
        for _ in 0..500 {
            clock = match random.below(10) {
                0 => (clock - random.below(furthest_back + 1)).max(latest_seen - furthest_back),
                1 => clock + random.below(7_200),
                _ => clock + random.below(90),
            };
            latest_seen = latest_seen.max(clock);
            let plan = match random.below(8) {
                0 => None,
                1 => Some(Arc::clone(&plans[1])),
                _ => Some(Arc::clone(&plans[0])),
            };
            if random.below(2) == 0 {
                closed.extend(windowing.hold(plan, time(clock)));
            } else {
                let recorded = windowing.record(plan, &get(), time(clock));
                closed.extend(recorded.closed);
                if matches!(recorded.outcome, CallOutcome::Counted { .. }) {
                    taken.push(clock);
                }
            }
        }
        closed.extend(windowing.hold(None, time(clock)));
        let mut stamps = Vec::new();
        let mut previous_end = 0;
        for written in closed.into_iter().map(write) {
            let (start, end) = (
                read_time(&written["window"]["start"]),
                read_time(&written["window"]["end"]),
            );
            let ats: Vec<u64> = written["sequence"]
                .as_array()
                .unwrap()
                .iter()
                .map(|step| read_time(&step["at"]))
                .collect();
            assert!(
                start <= end && end - start <= 60,
                "seed {seed}: {start} to {end}"
            );
            assert!(
                ats.is_sorted() && ats.iter().all(|at| (start..=end).contains(at)),
                "seed {seed}: {ats:?} in {start} to {end}"
            );
            if furthest_back <= 60 {
                assert!(
                    start >= previous_end,
                    "seed {seed}: {start} overlaps {previous_end}"
                );
            }
            previous_end = end;
            stamps.extend(ats);
        }
        // Windows close in the order calls are taken, so their steps are the calls in order.
        assert_eq!(
            stamps.len(),
            taken.len(),
            "seed {seed}: every call counted is in a window"
        );
        for (stamp, clock) in stamps.iter().zip(&taken) {
            assert!(
                (*clock..=clock + 60).contains(stamp),
                "seed {seed}: a call taken at {clock} stamped {stamp}"
            );
        }
    }
}
