use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rail_runtime::Clock;

// A clock whose two readings a test sets.
fn settable() -> (Clock, Arc<Mutex<(SystemTime, Duration)>>) {
    let readings = Arc::new(Mutex::new((
        UNIX_EPOCH + Duration::from_secs(1_800_000_000),
        Duration::from_secs(1000),
    )));
    let (wall, boot) = (Arc::clone(&readings), Arc::clone(&readings));
    let clock = Clock::new(
        move || wall.lock().unwrap().0,
        move || boot.lock().unwrap().1,
    );
    (clock, readings)
}

#[test]
fn an_age_is_the_larger_of_the_two_clocks_time() {
    // How far the wall and boot clocks move, as signed seconds, and the age that makes.
    for (wall, boot, age) in [
        // A freeze only the wall clock shows, then only the boot clock.
        (3600, 5, 3600),
        (5, 3600, 3600),
        // A wall clock stepped back doesn't make a value fresher.
        (-3600, 90, 90),
    ] {
        let (clock, readings) = settable();
        let fetched = clock.now();
        {
            let mut readings = readings.lock().unwrap();
            let by = |seconds: i64| Duration::from_secs(seconds.unsigned_abs());
            readings.0 = if wall < 0 {
                readings.0 - by(wall)
            } else {
                readings.0 + by(wall)
            };
            readings.1 += by(boot);
        }

        assert_eq!(
            clock.now().duration_since(fetched),
            Duration::from_secs(age),
            "{wall} {boot}"
        );
    }
}

#[test]
fn a_later_reading_is_no_age_at_all() {
    let (clock, readings) = settable();
    let earlier = clock.now();
    readings.lock().unwrap().1 += Duration::from_secs(10);
    let later = clock.now();

    assert_eq!(earlier.duration_since(later), Duration::ZERO);
}
