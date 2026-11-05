//! Its own binary: a test logging with no subscriber can race `tracing`'s one-time interest.

use std::sync::{Arc, Mutex};

use rail_runtime::Unavailable;

mod holder;

use holder::{Clock, HOUR, Script, start, ticket, token, until};

#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Logs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test(start_paused = true)]
async fn a_panicking_refresh_loop_is_logged() {
    let logs = Logs::default();
    let writer = logs.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    let _logging = tracing::subscriber::set_default(subscriber);
    let clock = Clock::new();
    let script = Script::new(&clock, |_| panic!("a defect in the fetch"));

    let holder = start(&clock, &script, HOUR);
    until(&clock, 1).await;

    assert_eq!(token(&holder), Err(Unavailable::IssuerUnreachable));
    let logs = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    assert!(
        logs.contains("the ticket refresh loop panicked; the ticket is no longer refreshed"),
        "{logs}"
    );
}

fn fingerprint(token: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(token.as_bytes())[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[tokio::test(start_paused = true)]
async fn a_lapsed_answer_kept_out_is_logged_by_its_own_fingerprint() {
    let logs = Logs::default();
    let writer = logs.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    let _logging = tracing::subscriber::set_default(subscriber);
    let clock = Clock::new();
    let (good, lapsed) = (clock.at(900), clock.at(0));
    let script = Script::new(&clock, move |n| {
        if n == 0 {
            ticket("a", good)
        } else {
            ticket("b", lapsed)
        }
    });

    let _holder = start(&clock, &script, HOUR);
    until(&clock, 451).await;

    let logs = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    let line = logs
        .lines()
        .find(|line| line.contains("returned a lapsed ticket; keeping the one held"))
        .unwrap_or_else(|| panic!("{logs}"));
    assert!(
        line.contains(&format!("lapsed=\"{}\"", fingerprint("b"))),
        "{line}"
    );
    assert!(
        line.contains(&format!("fingerprint=\"{}\"", fingerprint("a"))),
        "{line}"
    );
}
