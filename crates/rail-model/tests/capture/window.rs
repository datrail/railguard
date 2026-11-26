use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rail_model::body::{BodyRead, Unread};
use rail_model::capture::{Window, compile_plan, parse_plan};
use rail_model::request::ObservedRequest;
use serde_json::Value;

const PLANS: &str = include_str!("../../../../vectors/capture/plans.json");

fn time(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

#[test]
fn test_tells_what_a_closed_window_holds() {
    let plans: Value = serde_json::from_str(PLANS).unwrap();
    let mut plan = plans["plans"]["worked-example"].clone();
    plan["budget"]["max_unsent_windows"] = 3.into();
    let compiled = Arc::new(compile_plan(parse_plan(&plan.to_string()).unwrap()));
    let call = |path: &str| ObservedRequest {
        authority: b"api.github.com".to_vec(),
        method: b"GET".to_vec(),
        path: path.as_bytes().to_vec(),
        body: BodyRead::Unread(Unread::NotSent),
        body_bytes: 0,
    };
    // (paths called, calls dropped) and what the closed window holds: its records and steps,
    // and whether it's empty.
    let cases: [(&[&str], u64, usize, bool); 3] = [
        (&[], 0, 0, true),
        (&[], 1, 0, false),
        (&["/a", "/a", "/b"], 0, 5, false),
    ];
    for (paths, dropped, records_and_steps, empty) in cases {
        let mut window = Window::open(Arc::clone(&compiled), time(0));
        window.count_dropped(dropped);
        for path in paths {
            window.record(&call(path), time(1));
        }
        let closed = window.close(time(2));
        assert_eq!(closed.records_and_steps(), records_and_steps, "{paths:?}");
        assert_eq!(closed.is_empty(), empty, "{paths:?}, {dropped}");
    }
    let closed = Window::open(compiled, time(0)).close(time(1));
    assert_eq!(closed.max_unsent_windows(), 3);
}
