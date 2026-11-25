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

// Each plan's `window_seconds`, by index.
const WINDOW_SECONDS: [u64; 2] = [60, 300];

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

// Plans told apart by their version, one more than their index, with 60 s and 300 s windows.
fn plans() -> [Arc<CompiledPlan>; 2] {
    let vectors: Value = serde_json::from_str(VECTORS).unwrap();
    [0, 1].map(|index| {
        let mut plan = vectors["plan"].clone();
        plan["window_seconds"] = WINDOW_SECONDS[index].into();
        plan["plan_version"] = (index + 1).into();
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

// Seconds since the epoch of a time in January 1970, as a document writes one.
fn read_time(value: &Value) -> u64 {
    let text = value.as_str().unwrap();
    assert!(text.starts_with("1970-01-"), "{text}");
    let field = |at: usize| text[at..at + 2].parse::<u64>().unwrap();
    (field(8) - 1) * 86_400 + field(11) * 3_600 + field(14) * 60 + field(17)
}

// A closed window's plan index, start, end and steps' times, read from its document.
fn read(closed: ClosedWindow) -> (usize, u64, u64, Vec<u64>, u64) {
    let written = serde_json::to_value(closed.into_document(Uuid::nil(), time(0), 0)).unwrap();
    let steps = written["sequence"].as_array().unwrap().iter();
    (
        written["plan_version"].as_u64().unwrap() as usize - 1,
        read_time(&written["window"]["start"]),
        read_time(&written["window"]["end"]),
        steps.map(|step| read_time(&step["at"])).collect(),
        written["truncated"]["calls_dropped"].as_u64().unwrap(),
    )
}

// `v<plan_version> <start>-<end>, <calls> calls, <dropped> dropped`.
fn describe(closed: ClosedWindow) -> String {
    let (plan, start, end, steps, dropped) = read(closed);
    format!(
        "v{} {start}-{end}, {} calls, {dropped} dropped",
        plan + 1,
        steps.len()
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
                "v1 20-60, 0 calls, 0 dropped",
                "v2 61-70, 0 calls, 0 dropped",
            ],
        ),
        (
            "a call past the end closes the window first",
            &[Hold(Some(0), 0), Record(Some(0), 60), Hold(None, 70)],
            &[
                "v1 0-60, 0 calls, 0 dropped",
                "v1 60-70, 1 calls, 0 dropped",
            ],
        ),
        (
            "a clock stepped back reads as the latest time written, across the plan gone",
            &[
                Hold(Some(0), 0),
                Record(Some(0), 50),
                Record(None, 40),
                Record(Some(0), 45),
                Hold(None, 55),
            ],
            &[
                "v1 0-50, 1 calls, 0 dropped",
                "call dropped",
                "v1 50-55, 1 calls, 0 dropped",
            ],
        ),
        (
            "a minute back is absorbed, across a plan change; further back, the window closes at its latest time and the next opens then",
            &[
                Hold(Some(0), 100),
                Record(Some(0), 110),
                Hold(Some(1), 50),
                Record(Some(1), 52),
                Hold(Some(1), 48),
                Hold(None, 55),
            ],
            &[
                "v1 100-110, 1 calls, 0 dropped",
                "v2 110-110, 1 calls, 0 dropped",
                "v2 48-55, 0 calls, 0 dropped",
            ],
        ),
        (
            "further back within the window's span, too",
            &[
                Hold(Some(1), 0),
                Record(Some(1), 200),
                Record(Some(1), 100),
                Hold(None, 110),
            ],
            &[
                "v2 0-200, 1 calls, 0 dropped",
                "v2 100-110, 1 calls, 0 dropped",
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
                "v1 0-30, 0 calls, 2 dropped",
                "call dropped",
                "v1 130-140, 0 calls, 0 dropped",
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

// Over generated plans, calls and clocks stepped forward and back: each window's calls in order
// within its span, no window longer than its plan's, none overlapping the one before unless the
// clock stepped back more than a minute since, and each call counted under the plan held with it,
// stamped at its clock or up to a minute later.
#[test]
fn test_windows_hold_every_call_in_order_within_their_span_whatever_the_clock_does() {
    let plans = plans();
    for seed in 0..50 {
        let mut random = Random(seed);
        let mut windowing = Windowing::default();
        let (mut clock, mut latest_seen, mut resets) = (200_000, 200_000, 0);
        // Each window closed, with the steps back past a minute before and after its closing.
        let mut closed = Vec::new();
        // Each call counted: the clock it was taken at and its plan.
        let mut taken = Vec::new();
        for _ in 0..200 {
            clock = match random.below(10) {
                0 => clock - random.below(61),
                1 => clock - random.below(7_200),
                2 => clock + random.below(7_200),
                _ => clock + random.below(90),
            };
            let before = resets;
            resets += usize::from(clock + 60 < latest_seen);
            latest_seen = latest_seen.max(clock);
            let plan = [None, Some(1), Some(0), Some(0)][random.below(4) as usize];
            let held = plan.map(|plan: usize| Arc::clone(&plans[plan]));
            let window = if random.below(2) == 0 {
                windowing.hold(held, time(clock))
            } else {
                let recorded = windowing.record(held, &get(), time(clock));
                if let (CallOutcome::Counted { .. }, Some(plan)) = (recorded.outcome, plan) {
                    taken.push((clock, plan));
                }
                recorded.closed
            };
            closed.extend(window.map(|window| (window, before, resets)));
        }
        closed.extend(
            windowing
                .hold(None, time(clock))
                .map(|window| (window, resets, resets)),
        );
        let mut stamps = Vec::new();
        let mut previous: Option<(u64, usize)> = None;
        for (window, before, after) in closed {
            let (plan, start, end, steps, _) = read(window);
            assert!(
                start <= end && end - start <= WINDOW_SECONDS[plan],
                "seed {seed}: {start} to {end}"
            );
            assert!(
                steps.is_sorted() && steps.iter().all(|at| (start..=end).contains(at)),
                "seed {seed}: {steps:?} in {start} to {end}"
            );
            if let Some((previous_end, previous_before)) = previous
                && after == previous_before
            {
                assert!(
                    start >= previous_end,
                    "seed {seed}: {start} overlaps {previous_end}"
                );
            }
            previous = Some((end, before));
            stamps.extend(steps.into_iter().map(|at| (at, plan)));
        }
        // Windows close in the order calls are taken, so their steps are the calls in order.
        assert_eq!(
            stamps.len(),
            taken.len(),
            "seed {seed}: every call counted is in a window"
        );
        for ((stamp, plan), (clock, held)) in stamps.into_iter().zip(taken) {
            assert_eq!(
                plan, held,
                "seed {seed}: a call taken at {clock} under the plan held"
            );
            assert!(
                (clock..=clock + 60).contains(&stamp),
                "seed {seed}: a call taken at {clock} stamped {stamp}"
            );
        }
    }
}
