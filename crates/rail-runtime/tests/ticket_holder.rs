use std::time::Duration;

use rail_runtime::{TicketAnswer, Unavailable};
use tokio::time::sleep;

use crate::holder::{Clock, HOUR, Script, failed, start, ticket, token, until};

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

#[tokio::test(start_paused = true)]
async fn a_new_ticket_replaces_one_still_valid() {
    let clock = Clock::new();
    let (first, second) = (clock.at(900), clock.at(1800));
    let script = Script::new(&clock, move |n| {
        if n == 0 {
            ticket("a", first)
        } else {
            ticket("b", second)
        }
    });

    let holder = start(&clock, &script, HOUR);
    until(&clock, 450).await;

    assert_eq!(token(&holder), Ok("b".to_owned()));
    until(&clock, 1125).await;
    // From 450, half of b's remaining 1350 s.
    assert_eq!(script.fetched_at(), [0, 450, 1125]);
}

#[tokio::test(start_paused = true)]
async fn a_lapsed_answer_while_a_ticket_holds_leaves_the_ramp_alone() {
    let clock = Clock::new();
    let (good, lapsed) = (clock.at(40), clock.at(0));
    let script = Script::new(&clock, move |n| {
        if n == 0 {
            ticket("a", good)
        } else {
            ticket("b", lapsed)
        }
    });

    let _holder = start(&clock, &script, HOUR);
    until(&clock, 60).await;

    // Held until 40, lapsed answers kept out; at 40 the lapsed one is stored and the ramp starts.
    assert_eq!(script.fetched_at(), [0, 20, 30, 35, 40, 50]);
}

#[tokio::test(start_paused = true)]
async fn the_clock_is_read_for_the_fetch_as_it_is_sent() {
    let clock = Clock::new();
    let expiry = clock.at(900);
    let script = Script::new(&clock, move |_| ticket("a", expiry));

    let _holder = start(&clock, &script, HOUR);
    until(&clock, 450).await;

    assert_eq!(
        *script.sent_at.lock().unwrap(),
        [clock.at(0), clock.at(450)]
    );
}

#[tokio::test(start_paused = true)]
async fn a_lapsed_ticket_after_none_is_expired_not_not_found() {
    let clock = Clock::new();
    let lapsed = clock.at(0);
    let script = Script::new(&clock, move |n| {
        if n == 0 {
            Ok(TicketAnswer::NoTicket)
        } else {
            ticket("a", lapsed)
        }
    });

    let holder = start(&clock, &script, HOUR);
    until(&clock, 10).await;

    assert_eq!(token(&holder), Err(Unavailable::Expired));
}

#[tokio::test(start_paused = true)]
async fn a_new_ticket_replaces_a_lapsed_one() {
    let clock = Clock::new();
    let (lapsed, good) = (clock.at(0), clock.at(900));
    let script = Script::new(&clock, move |n| {
        if n == 0 {
            ticket("a", lapsed)
        } else {
            ticket("b", good)
        }
    });

    let holder = start(&clock, &script, HOUR);
    until(&clock, 0).await;
    assert_eq!(token(&holder), Err(Unavailable::Expired));
    until(&clock, 10).await;

    assert_eq!(token(&holder), Ok("b".to_owned()));
}

#[tokio::test(start_paused = true)]
async fn the_same_ticket_again_takes_its_new_expiry_lapsed_or_not() {
    let clock = Clock::new();
    let (soon, later) = (clock.at(100), clock.at(1000));
    let valid = Script::new(&clock, move |n| {
        ticket("a", if n == 0 { soon } else { later })
    });
    let lapsed = clock.at(0);
    let renewed = Script::new(&clock, move |n| {
        ticket("a", if n == 0 { lapsed } else { later })
    });

    let valid = start(&clock, &valid, HOUR);
    let renewed = start(&clock, &renewed, HOUR);
    until(&clock, 150).await;

    // Refetched at 50 and at 10, each past the expiry it first had.
    assert_eq!(token(&valid), Ok("a".to_owned()));
    assert_eq!(token(&renewed), Ok("a".to_owned()));
}

#[tokio::test(start_paused = true)]
async fn a_ticket_lapsing_as_it_arrives_is_lapsed_and_backs_off() {
    let clock = Clock::new();
    let script = Script::new(&clock, {
        let clock = clock.wall();
        move |_| ticket("a", clock())
    });

    let holder = start(&clock, &script, HOUR);
    until(&clock, 30).await;

    assert_eq!(token(&holder), Err(Unavailable::Expired));
    assert_eq!(script.fetched_at(), [0, 10, 30]);
}
