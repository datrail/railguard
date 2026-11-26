use rail_runtime::{Level, read_capture_plan_refresh};
use rail_test_support::describe;

const SETTING: &str = "RAIL_CAPTURE_PLAN_REFRESH_SECONDS";

#[test]
fn the_refresh_is_read_and_described_as_used() {
    assert_eq!(
        describe(&[(SETTING, " 600\n")], |settings| {
            read_capture_plan_refresh(settings, true)
        }),
        [(SETTING, "600".to_owned(), Level::Info)]
    );
}
