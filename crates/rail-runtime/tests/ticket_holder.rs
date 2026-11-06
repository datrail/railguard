use std::time::Duration;

use rail_runtime::{Jitter, TicketAnswer, Unavailable};
use tokio::time::sleep;

use crate::holder::{Clock, HOUR, Script, failed, start, start_jittered, ticket, token, until};

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
async fn a_failed_fetch_keeps_a_ticket_still_valid_until_it_lapses_and_expired_after() {
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
    // A fetch after the lapse fails: the lapsed ticket is kept, so it still reads as expired.
    until(&clock, 110).await;

    assert!(script.fetched_at().iter().any(|&at| at > 100));
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

    // 5, 10, 20, 40, 80, 160, 320, then 320 again: six doublings of 5 s.
    assert_eq!(
        script.fetched_at(),
        [0, 5, 15, 35, 75, 155, 315, 635, 955, 1275, 1595, 1915]
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

    assert_eq!(capped.fetched_at(), [0, 5, 15, 35, 65, 95, 125]);
    assert_eq!(floored.fetched_at()[..4], [0, 5, 10, 15]);
}

#[tokio::test(start_paused = true)]
async fn no_ticket_backs_off_like_a_failure() {
    let clock = Clock::new();
    let script = Script::new(&clock, |_| Ok(TicketAnswer::NoTicket));

    let _holder = start(&clock, &script, HOUR);
    until(&clock, 35).await;

    assert_eq!(script.fetched_at(), [0, 5, 15, 35]);
}

#[tokio::test(start_paused = true)]
async fn rail_center_handing_out_lapsed_tickets_still_backs_off() {
    let clock = Clock::new();
    let lapsed = clock.at(0);
    let script = Script::new(&clock, move |_| ticket("a", lapsed));

    let holder = start(&clock, &script, HOUR);
    until(&clock, 35).await;

    assert_eq!(script.fetched_at(), [0, 5, 15, 35]);
    assert_eq!(token(&holder), Err(Unavailable::Expired));
}

#[tokio::test(start_paused = true)]
async fn failures_while_a_ticket_holds_back_off_within_half_its_remaining_life() {
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
    until(&clock, 123).await;

    // From 20, waits 5, then 7.5 (half of 15, under 10), then the floor twice; lapsed by 42.5, the
    // fifth failure, so the ramp's 80 s.
    assert_eq!(script.fetched_at(), [0, 20, 25, 32, 37, 42, 122]);
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
    until(&clock, 75).await;

    // Fails at 0, 5 and 15; at 35 a ticket lapsing within the floor; then the ramp from 5 again.
    assert_eq!(script.fetched_at(), [0, 5, 15, 35, 40, 45, 55, 75]);
}

#[tokio::test(start_paused = true)]
async fn dropping_the_holder_stops_the_refreshes() {
    let clock = Clock::new();
    let script = Script::new(&clock, |_| failed());

    let holder = start(&clock, &script, HOUR);
    until(&clock, 5).await;
    drop(holder);
    sleep(HOUR).await;

    assert_eq!(script.fetched_at(), [0, 5]);
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
async fn a_lapsed_answer_never_replaces_a_ticket_still_valid_and_backs_off() {
    let clock = Clock::new();
    let (good, lapsed) = (clock.at(40), clock.at(0));
    let script = Script::new(&clock, move |n| {
        if n == 0 {
            ticket("a", good)
        } else {
            ticket("b", lapsed)
        }
    });

    let holder = start(&clock, &script, HOUR);
    until(&clock, 39).await;
    assert_eq!(token(&holder), Ok("a".to_owned()));
    until(&clock, 60).await;

    assert_eq!(token(&holder), Err(Unavailable::Expired));
    // Lapsed answers kept out until 40, each a failure, as in the test above.
    assert_eq!(script.fetched_at(), [0, 20, 25, 32, 37, 42]);
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
async fn the_period_runs_from_the_start_of_a_slow_fetch_and_never_under_5_s_after_it_ends() {
    let clock = Clock::new();
    let expiry = clock.at(86_400);
    let slow = Script {
        takes: Duration::from_secs(10),
        ..Script::new(&clock, move |_| ticket("a", expiry))
    };
    let slower = Script {
        takes: Duration::from_secs(598),
        ..Script::new(&clock, move |_| ticket("a", expiry))
    };

    let _slow = start(&clock, &slow, Duration::from_secs(600));
    let _slower = start(&clock, &slower, Duration::from_secs(600));
    until(&clock, 1206).await;

    assert_eq!(slow.fetched_at(), [0, 600, 1200]);
    assert_eq!(slower.fetched_at(), [0, 603, 1206]);
}

#[tokio::test(start_paused = true)]
async fn jitter_scales_every_wait_and_is_held_to_0_8_to_1_2() {
    let clock = Clock::new();
    let expiry = clock.at(86_400);
    let succeeding = move |n: usize| {
        if n == 0 {
            failed()
        } else {
            ticket("a", expiry)
        }
    };
    let cases: [(f64, [u64; 4]); 5] = [
        // A failure's 5 s wait floored at 5, a period of 600 s scaled.
        (0.8, [0, 5, 485, 965]),
        (1.2, [0, 6, 726, 1446]),
        (0.0, [0, 5, 485, 965]),
        (f64::NAN, [0, 5, 485, 965]),
        (5.0, [0, 6, 726, 1446]),
    ];

    let scripts = cases.map(|(factor, _)| {
        let script = Script::new(&clock, succeeding);
        let holder = start_jittered(
            &clock,
            &script,
            Duration::from_secs(600),
            Jitter::new(move || factor),
        );
        (script, holder)
    });
    until(&clock, 1446).await;

    for ((factor, expected), (script, _)) in cases.iter().zip(&scripts) {
        assert_eq!(script.fetched_at()[..4], *expected, "{factor}");
    }
}

#[tokio::test(start_paused = true)]
async fn random_jitter_varies_within_a_fifth_of_the_period() {
    let clock = Clock::new();
    let expiry = clock.at(86_400 * 30);
    let script = Script::new(&clock, move |_| ticket("a", expiry));

    let _holder = start_jittered(&clock, &script, Duration::from_secs(600), Jitter::random());
    until(&clock, 600 * 50).await;

    let fetched_at = script.fetched_at();
    let gaps: Vec<u64> = fetched_at
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .collect();
    assert!(gaps.len() >= 40, "{gaps:?}");
    // Whole seconds, so a gap may read one under.
    assert!(gaps.iter().all(|gap| (479..=720).contains(gap)), "{gaps:?}");
    assert!(gaps.iter().any(|gap| *gap < 599), "{gaps:?}");
    assert!(gaps.iter().any(|gap| *gap > 601), "{gaps:?}");
}

#[tokio::test(start_paused = true)]
async fn half_the_remaining_life_runs_from_the_end_of_a_slow_fetch_and_isnt_jittered_past() {
    let clock = Clock::new();
    let expiry = clock.at(800);
    let script = Script {
        takes: Duration::from_secs(10),
        ..Script::new(&clock, move |_| ticket("a", expiry))
    };

    let _holder = start_jittered(&clock, &script, HOUR, Jitter::new(|| 1.2));
    until(&clock, 405).await;

    // Read at 10, 790 s left: half of it from there.
    assert_eq!(script.fetched_at(), [0, 405]);
}
