use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rail_runtime::{
    Secret, Ticket, TicketAnswer, TicketHolder, TicketSource, Unavailable, WallClock,
};
use tokio::time::{Instant, sleep};

const HOUR: Duration = Duration::from_secs(3600);

// A wall clock that moves with tokio's paused time, starting at a fixed instant.
struct Clock {
    started: Instant,
}

impl Clock {
    fn new() -> Self {
        Self {
            started: Instant::now(),
        }
    }

    fn wall(&self) -> WallClock {
        let started = self.started;
        Arc::new(move || UNIX_EPOCH + Duration::from_secs(1_800_000_000) + started.elapsed())
    }

    // The wall-clock time `seconds` from the start.
    fn at(&self, seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_800_000_000 + seconds)
    }
}

type Answer = Result<TicketAnswer, String>;

// Answers fetch n with `answers(n)`, and records when each fetch came, in seconds from the start.
#[derive(Clone)]
struct Script {
    answers: Arc<dyn Fn(usize) -> Answer + Send + Sync>,
    fetched_at: Arc<Mutex<Vec<u64>>>,
    started: Instant,
}

impl Script {
    fn new(clock: &Clock, answers: impl Fn(usize) -> Answer + Send + Sync + 'static) -> Self {
        Self {
            answers: Arc::new(answers),
            fetched_at: Arc::default(),
            started: clock.started,
        }
    }

    fn fetched_at(&self) -> Vec<u64> {
        self.fetched_at.lock().unwrap().clone()
    }
}

impl TicketSource for Script {
    async fn fetch(&self, _sent_at: SystemTime) -> Answer {
        let mut fetched_at = self.fetched_at.lock().unwrap();
        fetched_at.push(self.started.elapsed().as_secs());
        (self.answers)(fetched_at.len() - 1)
    }
}

fn ticket(token: &str, expires_at: SystemTime) -> Answer {
    Ok(TicketAnswer::Ticket(Ticket {
        token: Secret::new(token.to_owned()),
        expires_at,
    }))
}

fn failed() -> Answer {
    Err("Rail Center was silent for 10s".to_owned())
}

fn start(clock: &Clock, script: &Script, refresh_interval: Duration) -> TicketHolder {
    TicketHolder::start(script.clone(), refresh_interval, clock.wall())
}

fn token(holder: &TicketHolder) -> Result<String, Unavailable> {
    holder
        .current()
        .map(|ticket| ticket.token.expose().to_owned())
}

// Lets the refresh loop run up to `seconds` from the start.
async fn until(clock: &Clock, seconds: u64) {
    tokio::time::sleep_until(clock.started + Duration::from_secs(seconds)).await;
    tokio::task::yield_now().await;
}

#[tokio::test(start_paused = true)]
async fn a_fetched_ticket_is_presented_from_the_first_fetch() {
    let clock = Clock::new();
    let expiry = clock.at(900);
    let script = Script::new(&clock, move |_| ticket("a", expiry));

    let holder = start(&clock, &script, HOUR);
    assert_eq!(token(&holder), Err(Unavailable::IssuerUnreachable));
    until(&clock, 0).await;

    assert_eq!(token(&holder), Ok("a".to_owned()));
    assert_eq!(script.fetched_at(), [0]);
}

#[tokio::test(start_paused = true)]
async fn a_failed_fetch_keeps_a_ticket_still_valid_until_it_lapses() {
    let clock = Clock::new();
    let expiry = clock.at(100);
    let script = Script::new(&clock, move |n| {
        if n == 0 {
            ticket("a", expiry)
        } else {
            failed()
        }
    });

    let holder = start(&clock, &script, HOUR);
    until(&clock, 99).await;
    assert_eq!(token(&holder), Ok("a".to_owned()));
    until(&clock, 100).await;

    assert_eq!(token(&holder), Err(Unavailable::Expired));
}

#[tokio::test(start_paused = true)]
async fn rail_center_answering_none_clears_a_ticket_still_valid() {
    let clock = Clock::new();
    let expiry = clock.at(900);
    let script = Script::new(&clock, move |n| {
        if n == 0 {
            ticket("a", expiry)
        } else {
            Ok(TicketAnswer::NoTicket)
        }
    });

    let holder = start(&clock, &script, HOUR);
    until(&clock, 450).await;

    assert_eq!(token(&holder), Err(Unavailable::NotFound));
}

#[tokio::test(start_paused = true)]
async fn not_found_lasts_only_while_rail_center_says_so() {
    let clock = Clock::new();
    let script = Script::new(&clock, |n| {
        if n == 0 {
            Ok(TicketAnswer::NoTicket)
        } else {
            failed()
        }
    });

    let holder = start(&clock, &script, HOUR);
    until(&clock, 0).await;
    assert_eq!(token(&holder), Err(Unavailable::NotFound));
    until(&clock, 10).await;

    assert_eq!(token(&holder), Err(Unavailable::IssuerUnreachable));
}

#[tokio::test(start_paused = true)]
async fn a_recovered_ticket_clears_the_reason() {
    let clock = Clock::new();
    let expiry = clock.at(900);
    let script = Script::new(&clock, move |n| match n {
        0 => Ok(TicketAnswer::NoTicket),
        _ => ticket("a", expiry),
    });

    let holder = start(&clock, &script, HOUR);
    until(&clock, 10).await;

    assert_eq!(token(&holder), Ok("a".to_owned()));
}

#[tokio::test(start_paused = true)]
async fn a_ticket_already_lapsed_never_replaces_one_still_valid() {
    let clock = Clock::new();
    let (good, lapsed) = (clock.at(900), clock.at(0));
    let script = Script::new(&clock, move |n| {
        if n == 0 {
            ticket("a", good)
        } else {
            ticket("b", lapsed)
        }
    });

    let holder = start(&clock, &script, HOUR);
    until(&clock, 450).await;
    assert_eq!(token(&holder), Ok("a".to_owned()));
    until(&clock, 900).await;

    assert_eq!(token(&holder), Err(Unavailable::Expired));
}

#[tokio::test(start_paused = true)]
async fn refreshes_come_at_half_the_remaining_life() {
    let clock = Clock::new();
    let expiry = clock.at(800);
    let script = Script::new(&clock, move |_| ticket("a", expiry));

    let _holder = start(&clock, &script, HOUR);
    until(&clock, 790).await;

    assert_eq!(script.fetched_at(), [0, 400, 600, 700, 750, 775, 787]);
}

#[tokio::test(start_paused = true)]
async fn a_short_ticket_is_refreshed_no_more_than_every_5_s() {
    let clock = Clock::new();
    let script = Script::new(&clock, {
        let clock = clock.wall();
        move |_| ticket("a", clock() + Duration::from_secs(2))
    });

    let _holder = start(&clock, &script, HOUR);
    until(&clock, 20).await;

    assert_eq!(script.fetched_at(), [0, 5, 10, 15, 20]);
}

#[tokio::test(start_paused = true)]
async fn a_long_ticket_is_still_refreshed_at_the_interval() {
    let clock = Clock::new();
    let expiry = clock.at(86_400);
    let script = Script::new(&clock, move |_| ticket("a", expiry));

    let _holder = start(&clock, &script, Duration::from_secs(600));
    until(&clock, 1800).await;

    assert_eq!(script.fetched_at(), [0, 600, 1200, 1800]);
}

#[tokio::test(start_paused = true)]
async fn repeated_failure_backs_off_to_a_ceiling() {
    let clock = Clock::new();
    let script = Script::new(&clock, |_| failed());

    let _holder = start(&clock, &script, HOUR);
    until(&clock, 1915).await;

    // 10, 20, 40, 80, 160, 320, then 320 again: six doublings of 5 s.
    assert_eq!(
        script.fetched_at(),
        [0, 10, 30, 70, 150, 310, 630, 950, 1270, 1590, 1910]
    );
}

#[tokio::test(start_paused = true)]
async fn the_backoff_never_exceeds_the_interval_nor_goes_under_5_s() {
    let clock = Clock::new();
    let capped = Script::new(&clock, |_| failed());
    let floored = Script::new(&clock, |_| failed());

    let _capped = start(&clock, &capped, Duration::from_secs(30));
    let _floored = start(&clock, &floored, Duration::from_secs(1));
    until(&clock, 130).await;

    assert_eq!(capped.fetched_at(), [0, 10, 30, 60, 90, 120]);
    assert_eq!(floored.fetched_at()[..4], [0, 5, 10, 15]);
}

#[tokio::test(start_paused = true)]
async fn no_ticket_backs_off_like_a_failure() {
    let clock = Clock::new();
    let script = Script::new(&clock, |_| Ok(TicketAnswer::NoTicket));

    let _holder = start(&clock, &script, HOUR);
    until(&clock, 70).await;

    assert_eq!(script.fetched_at(), [0, 10, 30, 70]);
}

#[tokio::test(start_paused = true)]
async fn rail_center_handing_out_lapsed_tickets_still_backs_off() {
    let clock = Clock::new();
    let lapsed = clock.at(0);
    let script = Script::new(&clock, move |_| ticket("a", lapsed));

    let holder = start(&clock, &script, HOUR);
    until(&clock, 70).await;

    assert_eq!(script.fetched_at(), [0, 10, 30, 70]);
    assert_eq!(token(&holder), Err(Unavailable::Expired));
}

#[tokio::test(start_paused = true)]
async fn failures_while_a_ticket_holds_leave_the_ramp_alone() {
    let clock = Clock::new();
    let expiry = clock.at(40);
    let script = Script::new(&clock, move |n| {
        if n == 0 {
            ticket("a", expiry)
        } else {
            failed()
        }
    });

    let _holder = start(&clock, &script, HOUR);
    until(&clock, 60).await;

    // Held until 40: half-lives 20, 10, then the 5 s floor. Lapsed at 40: the ramp starts at 10.
    assert_eq!(script.fetched_at(), [0, 20, 30, 35, 40, 50]);
}

#[tokio::test(start_paused = true)]
async fn a_success_restarts_the_ramp() {
    let clock = Clock::new();
    let script = Script::new(&clock, {
        let clock = clock.wall();
        move |n| match n {
            3 => ticket("a", clock() + Duration::from_secs(1)),
            _ => failed(),
        }
    });

    let _holder = start(&clock, &script, HOUR);
    until(&clock, 90).await;

    // Fails at 0, 10 and 30; at 70 a ticket lapsing within the floor; then the ramp from 10 again.
    assert_eq!(script.fetched_at(), [0, 10, 30, 70, 75, 85]);
}

#[tokio::test(start_paused = true)]
async fn dropping_the_holder_stops_the_refreshes() {
    let clock = Clock::new();
    let script = Script::new(&clock, |_| failed());

    let holder = start(&clock, &script, HOUR);
    until(&clock, 10).await;
    drop(holder);
    sleep(HOUR).await;

    assert_eq!(script.fetched_at(), [0, 10]);
}
