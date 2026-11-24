use std::time::{Duration, UNIX_EPOCH};

use rail_model::timestamp::Timestamp;

#[test]
fn test_writes_a_year_from_0_to_9999() {
    // Year 0's start, and 10000's, as seconds from 1970.
    let year_0 = Duration::from_secs(62_167_219_200);
    let year_10000 = Duration::from_secs(253_402_300_800);
    for (at, written) in [
        (
            UNIX_EPOCH - Duration::from_secs(1),
            "\"1969-12-31T23:59:59Z\"",
        ),
        (UNIX_EPOCH - year_0, "\"0000-01-01T00:00:00Z\""),
        (
            UNIX_EPOCH + year_10000 - Duration::from_nanos(1),
            "\"9999-12-31T23:59:59.999999999Z\"",
        ),
    ] {
        assert_eq!(serde_json::to_string(&Timestamp(at)).unwrap(), written);
    }
    for at in [
        UNIX_EPOCH - year_0 - Duration::from_secs(1),
        UNIX_EPOCH + year_10000,
    ] {
        assert!(serde_json::to_string(&Timestamp(at)).is_err(), "{at:?}");
    }
}
