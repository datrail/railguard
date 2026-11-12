use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rail_envoy::check_proxy;
use rail_grpc_host::RoleState;
use rail_runtime::{
    Jitter, OpaqueTicket, Secret, TicketAnswer, TicketHolder, TicketSource, WallClock,
};
use rail_test_support::wait_until;
use tokio::runtime::Handle;
use tokio::sync::Semaphore;

const START: u64 = 1_800_000_000;

// Answers each fetch only once the test lets it, the first with `first`, every later one failing.
struct Gated {
    gate: Arc<Semaphore>,
    first: fn() -> TicketAnswer,
    fetches: AtomicU64,
}

impl TicketSource for Gated {
    async fn fetch(&self, _sent_at: SystemTime) -> Result<TicketAnswer, String> {
        self.gate.acquire().await.unwrap().forget();
        match self.fetches.fetch_add(1, Ordering::SeqCst) {
            0 => Ok((self.first)()),
            _ => Err("unreachable".to_owned()),
        }
    }

    fn refresh_interval(&self) -> Duration {
        Duration::from_secs(3600)
    }
}

// Valid until a minute after the start.
fn a_ticket() -> TicketAnswer {
    TicketAnswer::Ticket(OpaqueTicket {
        token: Secret::new("t".to_owned()),
        expires_at: UNIX_EPOCH + Duration::from_secs(START + 60),
    })
}

// The holder, its gate, and the seconds its wall clock reads past the start.
fn start(first: fn() -> TicketAnswer) -> (Arc<TicketHolder>, Arc<Semaphore>, Arc<AtomicU64>) {
    let gate = Arc::new(Semaphore::new(0));
    let elapsed = Arc::new(AtomicU64::new(0));
    let read = Arc::clone(&elapsed);
    let now: WallClock =
        Arc::new(move || UNIX_EPOCH + Duration::from_secs(START + read.load(Ordering::SeqCst)));
    let source = Gated {
        gate: Arc::clone(&gate),
        first,
        fetches: AtomicU64::new(0),
    };
    let holder = TicketHolder::start(&Handle::current(), source, now, Jitter::new(|| 1.0));
    (Arc::new(holder), gate, elapsed)
}

fn not_ready(state: &'static str) -> RoleState {
    RoleState {
        ready: false,
        state,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn ready_only_while_a_valid_ticket_is_held() {
    let (holder, gate, elapsed) = start(a_ticket);
    let check = check_proxy(Some(holder));
    assert_eq!(check(), not_ready("issuer-unreachable"));

    gate.add_permits(1);
    tokio::task::block_in_place(|| wait_until(|| check().ready));
    assert_eq!(
        check(),
        RoleState {
            ready: true,
            state: "valid",
        }
    );

    // Its refresh fails, so the lapsed ticket is all there is.
    gate.add_permits(100);
    elapsed.store(60, Ordering::SeqCst);
    assert_eq!(check(), not_ready("expired"));
}

#[tokio::test(flavor = "multi_thread")]
async fn not_ready_while_rail_center_has_no_ticket_for_it() {
    let (holder, gate, _elapsed) = start(|| TicketAnswer::NoTicket);
    let check = check_proxy(Some(Arc::clone(&holder)));

    gate.add_permits(1);
    tokio::task::block_in_place(|| {
        wait_until(|| {
            holder
                .current()
                .is_err_and(|why| why.as_str() == "not-found")
        })
    });

    assert_eq!(check(), not_ready("not-found"));
}

#[test]
fn ready_at_once_with_rail_center_disabled() {
    assert_eq!(check_proxy(None)(), RoleState::DISABLED);
}
