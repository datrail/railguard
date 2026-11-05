use std::sync::Arc;
use std::time::{Duration, SystemTime};

use arc_swap::ArcSwap;
use sha2::{Digest, Sha256};
use tokio::runtime::Handle;
use tokio::task::AbortHandle;

use crate::ticket::{MIN_REFRESH_INTERVAL, OpaqueTicket, TicketAnswer, TicketSource};
use crate::xrail::Unavailable;

const MAX_BACKOFF_DOUBLINGS: u32 = 6;

/// The wall clock a ticket's `expires_at` is read against.
pub type WallClock = Arc<dyn Fn() -> SystemTime + Send + Sync>;

// What the answers so far leave to present.
#[derive(Debug, Default, PartialEq, Eq)]
enum Held {
    // No answer yet, or the last one was a failure with no ticket held.
    #[default]
    Unreached,
    // Set only by Rail Center answering "none", never by a failed fetch.
    NotFound,
    Ticket(Arc<OpaqueTicket>),
}

impl Held {
    fn ticket(&self) -> Option<&Arc<OpaqueTicket>> {
        match self {
            Self::Ticket(ticket) => Some(ticket),
            Self::Unreached | Self::NotFound => None,
        }
    }
}

/// Keeps a valid ticket to hand through Rail Center failures; dropping it stops the refreshes.
pub struct TicketHolder {
    held: Arc<ArcSwap<Held>>,
    now: WallClock,
    refresh: AbortHandle,
}

impl TicketHolder {
    /// Starts fetching at once, on `runtime`; the source's refresh interval caps the wait on a
    /// valid ticket.
    pub fn start(runtime: &Handle, source: impl TicketSource, now: WallClock) -> Self {
        let held = Arc::new(ArcSwap::from_pointee(Held::default()));
        let refresh = runtime.spawn(refresh(source, Arc::clone(&held), Arc::clone(&now)));
        let abort = refresh.abort_handle();
        // A panic ends the task quietly, and the ticket would lapse with nothing saying why.
        runtime.spawn(async move {
            if refresh.await.is_err_and(|error| error.is_panic()) {
                tracing::error!(
                    "the ticket refresh loop panicked; the ticket is no longer refreshed"
                );
            }
        });
        Self {
            held,
            now,
            refresh: abort,
        }
    }

    /// The ticket to present, or why there is none, from one read of the ticket and the clock.
    pub fn current(&self) -> Result<Arc<OpaqueTicket>, Unavailable> {
        match &**self.held.load() {
            Held::Ticket(ticket) if ticket.is_valid((self.now)()) => Ok(Arc::clone(ticket)),
            Held::Ticket(_) => Err(Unavailable::Expired),
            Held::NotFound => Err(Unavailable::NotFound),
            Held::Unreached => Err(Unavailable::IssuerUnreachable),
        }
    }
}

impl Drop for TicketHolder {
    fn drop(&mut self) {
        self.refresh.abort();
    }
}

async fn refresh(source: impl TicketSource, held: Arc<ArcSwap<Held>>, now: WallClock) {
    let refresh_interval = source.refresh_interval();
    // Attempts with no valid ticket held, so the ramp restarts when a ticket lapses.
    let mut failures = 0;
    loop {
        let answer = source.fetch(now()).await;
        let previous = held.load_full();
        let at = now();
        let (next, outcome) = step(&previous, answer, at, &mut failures);
        if let Some(next) = next {
            held.store(Arc::new(next));
        }
        let ticket = held.load().ticket().cloned();
        let delay = next_delay(ticket.as_deref(), failures, refresh_interval, now());
        log(&outcome, ticket.as_deref(), at, delay);
        tokio::time::sleep(delay).await;
    }
}

// What one answer, read at `at`, makes of the ticket held: what to hold next (`None` keeps it),
// and what to log. Counts in `failures` each attempt that leaves no valid ticket.
fn step(
    previous: &Held,
    answer: Result<TicketAnswer, String>,
    at: SystemTime,
    failures: &mut u32,
) -> (Option<Held>, Outcome) {
    let usable = |held: &Held| held.ticket().is_some_and(|ticket| ticket.is_valid(at));
    match answer {
        Ok(TicketAnswer::NoTicket) => {
            *failures += 1;
            (
                Some(Held::NotFound),
                Outcome::NoTicket {
                    cleared: usable(previous),
                },
            )
        }
        Err(reason) => {
            let kept = usable(previous);
            if !kept {
                *failures += 1;
            }
            let next = match previous.ticket() {
                Some(ticket) => Held::Ticket(Arc::clone(ticket)),
                None => Held::Unreached,
            };
            (Some(next), Outcome::Failed { reason, kept })
        }
        // An already-lapsed ticket never displaces one still good.
        Ok(TicketAnswer::Ticket(ticket)) if !ticket.is_valid(at) && usable(previous) => {
            let lapsed = fingerprint(ticket.token.expose());
            (None, Outcome::LapsedKept { lapsed })
        }
        Ok(TicketAnswer::Ticket(ticket)) => {
            let outcome = if !ticket.is_valid(at) {
                Outcome::Lapsed
            } else if previous.ticket().is_none() {
                Outcome::Acquired
            } else if previous
                .ticket()
                .is_some_and(|held| held.token != ticket.token)
            {
                Outcome::Rotated
            } else {
                Outcome::Unchanged
            };
            *failures = if ticket.is_valid(at) {
                0
            } else {
                *failures + 1
            };
            (Some(Held::Ticket(Arc::new(ticket))), outcome)
        }
    }
}

// What one fetch did to the ticket held, for the log.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Acquired,
    Rotated,
    Unchanged,
    NoTicket { cleared: bool },
    Failed { reason: String, kept: bool },
    // Not the one held, which is kept.
    LapsedKept { lapsed: String },
    Lapsed,
}

fn log(outcome: &Outcome, ticket: Option<&OpaqueTicket>, now: SystemTime, retry: Duration) {
    let fingerprint = ticket.map(|ticket| fingerprint(ticket.token.expose()));
    let fingerprint = fingerprint.as_deref().unwrap_or("none");
    let expires_in = ticket
        .and_then(|ticket| ticket.expires_at.duration_since(now).ok())
        .unwrap_or_default()
        .as_secs();
    let retry = retry.as_secs();
    match outcome {
        Outcome::Acquired => {
            tracing::info!(fingerprint, expires_in, "ticket acquired");
        }
        Outcome::Rotated => {
            tracing::info!(fingerprint, expires_in, "ticket rotated");
        }
        Outcome::Unchanged => {
            tracing::debug!(fingerprint, expires_in, "ticket refreshed, unchanged");
        }
        Outcome::NoTicket { cleared } => {
            tracing::warn!(
                cleared,
                retry,
                "Rail Center holds no ticket for this host and sandbox"
            );
        }
        Outcome::Failed { reason, kept: true } => {
            tracing::warn!(
                reason,
                fingerprint,
                expires_in,
                retry,
                "ticket refresh failed; keeping the one held"
            );
        }
        Outcome::Failed {
            reason,
            kept: false,
        } => {
            tracing::warn!(reason, retry, "ticket refresh failed; none is held");
        }
        Outcome::LapsedKept { lapsed } => {
            tracing::warn!(
                lapsed,
                fingerprint,
                expires_in,
                "Rail Center returned a lapsed ticket; keeping the one held"
            );
        }
        Outcome::Lapsed => {
            tracing::warn!(retry, "Rail Center returned a lapsed ticket; none is valid");
        }
    }
}

/// A short label for a ticket: shows that it rotated, never what it is.
fn fingerprint(token: &str) -> String {
    Sha256::digest(token.as_bytes())[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn next_delay(
    ticket: Option<&OpaqueTicket>,
    failures: u32,
    refresh_interval: Duration,
    now: SystemTime,
) -> Duration {
    let wait = match ticket.and_then(|ticket| ticket.expires_at.duration_since(now).ok()) {
        // Half the remaining life, so a short ticket is replaced before it lapses.
        Some(remaining) if !remaining.is_zero() => remaining / 2,
        // Nothing usable held: presenting fails closed, so retry soon, backing off.
        _ => MIN_REFRESH_INTERVAL * 2_u32.pow(failures.min(MAX_BACKOFF_DOUBLINGS)),
    };
    wait.min(refresh_interval).max(MIN_REFRESH_INTERVAL)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use super::{Held, Outcome, fingerprint, step};
    use crate::config::Secret;
    use crate::ticket::{OpaqueTicket, TicketAnswer};

    const AT: u64 = 1_800_000_000;

    fn at() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(AT)
    }

    // A ticket `token` that lapses `seconds` after `at`, before it if negative.
    fn ticket(token: &str, seconds: i64) -> OpaqueTicket {
        OpaqueTicket {
            token: Secret::new(token.to_owned()),
            expires_at: UNIX_EPOCH + Duration::from_secs(AT.saturating_add_signed(seconds)),
        }
    }

    fn held(token: &str, seconds: i64) -> Held {
        Held::Ticket(Arc::new(ticket(token, seconds)))
    }

    fn answer(token: &str, seconds: i64) -> Result<TicketAnswer, String> {
        Ok(TicketAnswer::Ticket(ticket(token, seconds)))
    }

    fn failed() -> Result<TicketAnswer, String> {
        Err("silent".to_owned())
    }

    // What `answer` makes of `previous`, and the failure count after one before it.
    fn run(previous: &Held, answer: Result<TicketAnswer, String>) -> (Option<Held>, Outcome, u32) {
        let mut failures = 1;
        let (next, outcome) = step(previous, answer, at(), &mut failures);
        (next, outcome, failures)
    }

    #[test]
    fn a_valid_ticket_is_acquired_whatever_came_before_and_resets_the_failures() {
        for previous in [Held::Unreached, Held::NotFound] {
            assert_eq!(
                run(&previous, answer("a", 60)),
                (Some(held("a", 60)), Outcome::Acquired, 0)
            );
        }
    }

    #[test]
    fn a_new_token_rotates_the_one_held_lapsed_or_not() {
        for previous in [held("a", 60), held("a", -60)] {
            assert_eq!(
                run(&previous, answer("b", 120)),
                (Some(held("b", 120)), Outcome::Rotated, 0)
            );
        }
    }

    #[test]
    fn the_same_token_again_is_unchanged_with_its_new_expiry_lapsed_or_not() {
        for previous in [held("a", 60), held("a", -60)] {
            assert_eq!(
                run(&previous, answer("a", 120)),
                (Some(held("a", 120)), Outcome::Unchanged, 0)
            );
        }
    }

    #[test]
    fn no_ticket_clears_the_one_held_and_counts_as_a_failure() {
        let no_ticket = || Ok(TicketAnswer::NoTicket);
        assert_eq!(
            run(&held("a", 60), no_ticket()),
            (Some(Held::NotFound), Outcome::NoTicket { cleared: true }, 2)
        );
        for previous in [Held::Unreached, Held::NotFound, held("a", -60)] {
            assert_eq!(
                run(&previous, no_ticket()),
                (
                    Some(Held::NotFound),
                    Outcome::NoTicket { cleared: false },
                    2
                )
            );
        }
    }

    #[test]
    fn a_failure_keeps_a_valid_ticket_without_counting() {
        assert_eq!(
            run(&held("a", 60), failed()),
            (
                Some(held("a", 60)),
                Outcome::Failed {
                    reason: "silent".to_owned(),
                    kept: true
                },
                1
            )
        );
    }

    #[test]
    fn a_failure_keeps_a_lapsed_ticket_and_forgets_not_found() {
        let failed_unkept = || Outcome::Failed {
            reason: "silent".to_owned(),
            kept: false,
        };
        assert_eq!(
            run(&held("a", -60), failed()),
            (Some(held("a", -60)), failed_unkept(), 2)
        );
        for previous in [Held::Unreached, Held::NotFound] {
            assert_eq!(
                run(&previous, failed()),
                (Some(Held::Unreached), failed_unkept(), 2)
            );
        }
    }

    #[test]
    fn a_lapsed_ticket_never_displaces_a_valid_one() {
        assert_eq!(
            run(&held("a", 60), answer("b", -60)),
            (
                None,
                Outcome::LapsedKept {
                    lapsed: fingerprint("b")
                },
                1
            )
        );
    }

    #[test]
    fn a_lapsed_ticket_is_held_where_none_valid_is_and_counts_as_a_failure() {
        for previous in [Held::Unreached, Held::NotFound, held("a", -120)] {
            assert_eq!(
                run(&previous, answer("b", -60)),
                (Some(held("b", -60)), Outcome::Lapsed, 2)
            );
        }
    }

    #[test]
    fn a_ticket_lapsing_at_the_instant_read_is_lapsed() {
        assert_eq!(
            run(&Held::Unreached, answer("a", 0)),
            (Some(held("a", 0)), Outcome::Lapsed, 2)
        );
    }
}
