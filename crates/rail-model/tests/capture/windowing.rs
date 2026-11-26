use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rail_model::body::{BodyRead, Unread};
use rail_model::capture::{
    CallOutcome, ClosedWindow, CompiledPlan, Windowing, compile_plan, parse_plan,
};
use rail_model::request::ObservedRequest;
use serde_json::Value;
use uuid::Uuid;

const PLANS: &str = include_str!("../../../../vectors/capture/plans.json");

// Each plan's `window_seconds`, by index.
const WINDOW_SECONDS: [u64; 2] = [60, 300];

fn time(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

// Plans told apart by their version, one more than their index, with 60 s and 300 s windows.
fn plans() -> [Arc<CompiledPlan>; 2] {
    let plans: Value = serde_json::from_str(PLANS).unwrap();
    [0, 1].map(|index| {
        let mut plan = plans["plans"]["worked-example"].clone();
        plan["window_seconds"] = WINDOW_SECONDS[index].into();
        plan["plan_version"] = (index + 1).into();
        Arc::new(compile_plan(parse_plan(&plan.to_string()).unwrap()))
    })
}

fn call(method: &str, path: &str, why: Unread) -> ObservedRequest {
    ObservedRequest {
        authority: b"api.github.com".to_vec(),
        method: method.as_bytes().to_vec(),
        path: path.as_bytes().to_vec(),
        body: BodyRead::Unread(why),
        body_bytes: 0,
    }
}

fn get() -> ObservedRequest {
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

#[test]
fn test_a_window_whose_end_is_past_the_latest_time_ends_only_by_its_plan() {
    let [plan, _] = plans();
    let mut windowing = Windowing::default();
    let last = UNIX_EPOCH + Duration::from_secs(i64::MAX as u64 - 1);
    assert!(!windowing.is_open());
    assert!(windowing.hold(Some(Arc::clone(&plan)), last).is_none());
    assert!(windowing.is_open());
    assert_eq!(windowing.end(), None);
    let recorded = windowing.record(Some(plan), &get(), last);
    assert!(recorded.closed.is_none());
    assert!(matches!(recorded.outcome, CallOutcome::Counted { .. }));
    assert!(windowing.hold(None, last).is_some());
    assert!(!windowing.is_open());
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
