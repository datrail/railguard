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
fn an_age_counts_both_clocks_moving_together_once() {
    let (clock, readings) = settable();
    let fetched = clock.now();
    {
        let mut readings = readings.lock().unwrap();
        readings.0 += Duration::from_secs(60);
        readings.1 += Duration::from_secs(60);
    }

    assert_eq!(clock.now().duration_since(fetched), Duration::from_secs(60));
}

#[test]
fn a_freeze_the_wall_clock_shows_is_counted() {
    let (clock, readings) = settable();
    let fetched = clock.now();
    {
        let mut readings = readings.lock().unwrap();
        readings.0 += Duration::from_secs(3600);
        readings.1 += Duration::from_secs(5);
    }

    assert_eq!(
        clock.now().duration_since(fetched),
        Duration::from_secs(3600)
    );
}

#[test]
fn a_freeze_the_boot_clock_shows_is_counted() {
    let (clock, readings) = settable();
    let fetched = clock.now();
    {
        let mut readings = readings.lock().unwrap();
        readings.0 += Duration::from_secs(5);
        readings.1 += Duration::from_secs(3600);
    }

    assert_eq!(
        clock.now().duration_since(fetched),
        Duration::from_secs(3600)
    );
}

#[test]
fn a_wall_clock_stepped_back_does_not_make_a_value_fresher() {
    let (clock, readings) = settable();
    let fetched = clock.now();
    {
        let mut readings = readings.lock().unwrap();
        readings.0 -= Duration::from_secs(3600);
        readings.1 += Duration::from_secs(90);
    }

    assert_eq!(clock.now().duration_since(fetched), Duration::from_secs(90));
}

#[test]
fn a_later_reading_is_no_age_at_all() {
    let (clock, readings) = settable();
    let earlier = clock.now();
    readings.lock().unwrap().1 += Duration::from_secs(10);
    let later = clock.now();

    assert_eq!(earlier.duration_since(later), Duration::ZERO);
}

#[test]
fn the_system_clock_ages_as_time_passes() {
    let clock = Clock::system();
    let fetched = clock.now();
    std::thread::sleep(Duration::from_millis(20));

    let age = clock.now().duration_since(fetched);

    assert!(age >= Duration::from_millis(20), "{age:?}");
    assert!(age < Duration::from_secs(10), "{age:?}");
}
