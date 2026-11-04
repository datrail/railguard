use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use arc_swap::ArcSwap;
use tokio::task::JoinHandle;

use crate::client::RailCenterClient;
use crate::ticket::{Ticket, TicketAnswer, TicketQuery, fetch_ticket};

// Constants, not settings: a higher ratio presents tickets about to lapse, and a lower floor lets
// a fleet hammer Rail Center.
const MIN_REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const MAX_BACKOFF_DOUBLINGS: u32 = 6;

/// The wall clock a ticket's `expires_at` is read against.
pub type WallClock = Arc<dyn Fn() -> SystemTime + Send + Sync>;

/// Where tickets come from: Rail Center, or a test's script.
pub trait TicketSource: Send + Sync + 'static {
    /// One fetch; `sent_at` is the wall-clock time read just before it.
    fn fetch(
        &self,
        sent_at: SystemTime,
    ) -> impl Future<Output = Result<TicketAnswer, String>> + Send;
}

/// This host's ticket, fetched from Rail Center.
pub struct RailCenterTickets {
    pub client: RailCenterClient,
    pub query: TicketQuery,
}

impl TicketSource for RailCenterTickets {
    async fn fetch(&self, sent_at: SystemTime) -> Result<TicketAnswer, String> {
        fetch_ticket(&self.client, &self.query, sent_at).await
    }
}

/// Why no ticket is presented, as `x-rail-status` says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unavailable {
    /// Rail Center answered that this host and sandbox have no ticket.
    NotFound,
    /// The ticket held lapsed with no valid one to replace it.
    Expired,
    /// No answer yet, or the last answer was a failure to reach Rail Center.
    IssuerUnreachable,
}

impl Unavailable {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotFound => "not-found",
            Self::Expired => "expired",
            Self::IssuerUnreachable => "issuer-unreachable",
        }
    }
}

#[derive(Default)]
struct Held {
    ticket: Option<Arc<Ticket>>,
    // Set only by Rail Center answering "none", never by a failed fetch.
    not_found: bool,
}

/// Keeps a valid ticket to hand through Rail Center failures, refreshing it in the background
/// on the runtime it was started on. Dropping it stops the refreshes.
pub struct TicketHolder {
    held: Arc<ArcSwap<Held>>,
    now: WallClock,
    refresh: JoinHandle<()>,
}

impl TicketHolder {
    /// Starts fetching at once. `refresh_interval` caps the time between fetches of a valid
    /// ticket; the ticket's own lifetime paces them below that.
    pub fn start(source: impl TicketSource, refresh_interval: Duration, now: WallClock) -> Self {
        let held = Arc::new(ArcSwap::from_pointee(Held::default()));
        let refresh = tokio::spawn(refresh(
            source,
            refresh_interval,
            Arc::clone(&held),
            Arc::clone(&now),
        ));
        Self { held, now, refresh }
    }

    /// The ticket to present, or why there is none, from one read of the ticket and the clock.
    pub fn current(&self) -> Result<Arc<Ticket>, Unavailable> {
        let held = self.held.load();
        match &held.ticket {
            Some(ticket) if ticket.is_valid((self.now)()) => Ok(Arc::clone(ticket)),
            _ if held.not_found => Err(Unavailable::NotFound),
            Some(_) => Err(Unavailable::Expired),
            None => Err(Unavailable::IssuerUnreachable),
        }
    }
}

impl Drop for TicketHolder {
    fn drop(&mut self) {
        self.refresh.abort();
    }
}

async fn refresh(
    source: impl TicketSource,
    refresh_interval: Duration,
    held: Arc<ArcSwap<Held>>,
    now: WallClock,
) {
    // Attempts made with no valid ticket held, not Rail Center's failures: the ramp restarts
    // when a ticket lapses, rather than starting at its ceiling.
    let mut failures = 0;
    loop {
        let answer = source.fetch(now()).await;
        let previous = held.load_full();
        let at = now();
        let usable = |ticket: &Option<Arc<Ticket>>| ticket.as_ref().is_some_and(|t| t.is_valid(at));
        let next = match answer {
            Ok(TicketAnswer::NoTicket) => {
                failures += 1;
                Some(Held {
                    ticket: None,
                    not_found: true,
                })
            }
            Err(_) => {
                if !usable(&previous.ticket) {
                    failures += 1;
                }
                Some(Held {
                    ticket: previous.ticket.clone(),
                    not_found: false,
                })
            }
            // An already-lapsed ticket never displaces one still good.
            Ok(TicketAnswer::Ticket(ticket))
                if !ticket.is_valid(at) && usable(&previous.ticket) =>
            {
                None
            }
            Ok(TicketAnswer::Ticket(ticket)) => {
                failures = if ticket.is_valid(at) { 0 } else { failures + 1 };
                Some(Held {
                    ticket: Some(Arc::new(ticket)),
                    not_found: false,
                })
            }
        };
        if let Some(next) = next {
            held.store(Arc::new(next));
        }
        let ticket = held.load().ticket.clone();
        tokio::time::sleep(next_delay(
            ticket.as_deref(),
            failures,
            refresh_interval,
            now(),
        ))
        .await;
    }
}

fn next_delay(
    ticket: Option<&Ticket>,
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
