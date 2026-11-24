use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, UNIX_EPOCH};

use rail_envoy::check_proxy;
use rail_grpc_host::RoleState;
use rail_model::agent::AgentId;
use rail_runtime::{FetchError, Jitter, Response, TicketHolder, TicketSource, WallClock};
use rail_test_support::wait_until;
use reqwest::StatusCode;
use tokio::runtime::Handle;
use tokio::sync::Semaphore;

const START: u64 = 1_800_000_000;

// Answers each fetch only once the test lets it, the first with `first`'s tickets, every later one
// failing.
struct Gated {
    gate: Arc<Semaphore>,
    first: &'static str,
    fetches: AtomicU64,
    agent: AgentId,
}

impl TicketSource for Gated {
    async fn fetch(&self) -> Result<Response, FetchError> {
        self.gate.acquire().await.unwrap().forget();
        match self.fetches.fetch_add(1, Ordering::SeqCst) {
            0 => Ok(Response {
                status: StatusCode::OK,
                body: format!(r#"{{"host_id":"host-1","tickets":[{}]}}"#, self.first).into_bytes(),
            }),
            _ => Err(FetchError::Transport("unreachable".to_owned())),
        }
    }

    fn agent(&self) -> &AgentId {
        &self.agent
    }
}

// Valid until a minute after the start.
const A_TICKET: &str =
    r#"{"sandbox_name":"sandbox a","token":"t","expires_at":"2027-01-15T08:01:00Z"}"#;

// The holder, its gate, and the seconds its wall clock reads past the start.
fn start(first: &'static str) -> (Arc<TicketHolder>, Arc<Semaphore>, Arc<AtomicU64>) {
    let gate = Arc::new(Semaphore::new(0));
    let elapsed = Arc::new(AtomicU64::new(0));
    let read = Arc::clone(&elapsed);
    let now: WallClock =
        Arc::new(move || UNIX_EPOCH + Duration::from_secs(START + read.load(Ordering::SeqCst)));
    let source = Gated {
        gate: Arc::clone(&gate),
        first,
        fetches: AtomicU64::new(0),
        agent: AgentId {
            host_id: "host-1".to_owned(),
            sandbox_name: "sandbox a".to_owned(),
        },
    };
    let holder = TicketHolder::start(
        &Handle::current(),
        source,
        Duration::from_secs(3600),
        now,
        Jitter::new(|| 1.0),
    );
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
    let (holder, gate, elapsed) = start(A_TICKET);
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
    let (holder, gate, _elapsed) = start("");
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
