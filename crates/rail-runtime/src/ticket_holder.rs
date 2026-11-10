use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};
use tokio::runtime::Handle;

use crate::holder::{
    Holder, Jitter, Kind, MAX_BACKOFF, MIN_REFRESH_INTERVAL, Schedule, Step, round_to_seconds,
};
use crate::ticket::{OpaqueTicket, TicketAnswer, TicketSource};
use crate::xrail::Unavailable;

/// The wall clock: what a ticket's `expires_at` is read against, and denials are stamped with.
pub type WallClock = Arc<dyn Fn() -> SystemTime + Send + Sync>;

// What the answers so far leave to present.
#[derive(Default)]
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
    holder: Holder<Held>,
    now: WallClock,
}

impl TicketHolder {
    /// Starts fetching at once, on `runtime`; the source's refresh interval, jittered, caps the wait
    /// on a valid ticket, and `jitter` scales every wait.
    pub fn start(
        runtime: &Handle,
        source: impl TicketSource,
        now: WallClock,
        jitter: Jitter,
    ) -> Self {
        let refresh_interval = source.refresh_interval();
        let schedule = Schedule {
            period: refresh_interval,
            initial_delay: MIN_REFRESH_INTERVAL,
            max_delay: MAX_BACKOFF.min(refresh_interval),
        };
        let kind = Tickets {
            source,
            now: Arc::clone(&now),
        };
        let holder = Holder::start(runtime, kind, Held::default(), schedule, jitter);
        Self { holder, now }
    }

    /// The ticket to present, or why there is none, from one read of the ticket and the clock.
    pub fn current(&self) -> Result<Arc<OpaqueTicket>, Unavailable> {
        match &*self.holder.snapshot() {
            Held::Ticket(ticket) if ticket.is_valid((self.now)()) => Ok(Arc::clone(ticket)),
            Held::Ticket(_) => Err(Unavailable::Expired),
            Held::NotFound => Err(Unavailable::NotFound),
            Held::Unreached => Err(Unavailable::IssuerUnreachable),
        }
    }
}

struct Tickets<S> {
    source: S,
    now: WallClock,
}

impl<S: TicketSource> Kind for Tickets<S> {
    type Held = Held;
    type Answer = Result<TicketAnswer, String>;
    type Outcome = Outcome;

    const WHAT: &'static str = "ticket";

    fn fetch(&self) -> impl Future<Output = Self::Answer> + Send {
        self.source.fetch((self.now)())
    }

    fn step(&self, held: &Held, answer: Self::Answer) -> Step<Held, Outcome> {
        step(held, answer, (self.now)())
    }

    // Half the remaining life, so a short ticket is replaced before it lapses.
    fn wait_at_most(&self, held: &Held) -> Option<Duration> {
        held.ticket()
            .and_then(|ticket| ticket.expires_at.duration_since((self.now)()).ok())
            .map(|remaining| remaining / 2)
    }

    fn report(&self, outcome: Outcome, held: &Held, _: bool, retry: Duration) {
        log(
            &outcome,
            held.ticket().map(|ticket| &**ticket),
            (self.now)(),
            retry,
        );
    }
}

// What one answer, read at `at`, makes of the ticket held. Only a valid ticket is a success.
fn step(
    previous: &Held,
    answer: Result<TicketAnswer, String>,
    at: SystemTime,
) -> Step<Held, Outcome> {
    let usable = |held: &Held| held.ticket().is_some_and(|ticket| ticket.is_valid(at));
    let (next, succeeded, outcome) = match answer {
        Ok(TicketAnswer::NoTicket) => (
            Some(Held::NotFound),
            false,
            Outcome::NoTicket {
                cleared: usable(previous),
            },
        ),
        Err(reason) => {
            let kept = usable(previous);
            let next = match previous.ticket() {
                Some(ticket) => Held::Ticket(Arc::clone(ticket)),
                None => Held::Unreached,
            };
            (Some(next), false, Outcome::Failed { reason, kept })
        }
        // An already-lapsed ticket never displaces one still good.
        Ok(TicketAnswer::Ticket(ticket)) if !ticket.is_valid(at) && usable(previous) => {
            let lapsed = fingerprint(ticket.token.expose());
            (None, false, Outcome::LapsedKept { lapsed })
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
            let valid = ticket.is_valid(at);
            (Some(Held::Ticket(Arc::new(ticket))), valid, outcome)
        }
    };
    Step {
        next,
        succeeded,
        outcome,
    }
}

// What one fetch did to the ticket held, for the log.
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
    let retry = round_to_seconds(retry);
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
