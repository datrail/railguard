//! Call record documents, sent to Rail Center in the background, oldest first, one at a time.

use std::collections::VecDeque;
use std::future::Future;
use std::mem;
use std::pin::Pin;
use std::time::Duration;

use rail_model::capture::ClosedWindow;
use reqwest::StatusCode;
use ring::rand::{SecureRandom, SystemRandom};
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, sleep_until};
use uuid::Uuid;

use crate::client::{FetchError, RailCenterClient, answered};
use crate::clock::WallClock;
use crate::retry::{Backoff, Jitter, round_to_seconds};

const ROUTE: &str = "/v1/call-records";

// The upload and the wait for its status: a window at the plan's ceilings is tens of MiB.
const DEADLINE: Duration = Duration::from_secs(30);

// Bounds the queue's memory however a plan's ceilings combine: under two windows at them, each
// up to rail-model's 100 000 groups and 1 000 000 steps.
const MAX_QUEUED_RECORDS_AND_STEPS: usize = 2_000_000;

// How long the shutdown flush may take.
const FLUSH_LIMIT: Duration = Duration::from_secs(2);

/// Where documents go: Rail Center, or a test's script.
pub trait CallRecordSink: Send + Sync + 'static {
    /// One attempt to deliver `body`, answered by its status: what each means is the reporter's.
    fn send(&self, body: Vec<u8>) -> impl Future<Output = Result<StatusCode, FetchError>> + Send;
}

/// Documents sent to Rail Center.
pub struct RailCenterCallRecords {
    pub client: RailCenterClient,
}

impl CallRecordSink for RailCenterCallRecords {
    async fn send(&self, body: Vec<u8>) -> Result<StatusCode, FetchError> {
        self.client.post_for_status(ROUTE, body, DEADLINE).await
    }
}

enum Message {
    Window(Box<ClosedWindow>),
    SendNow,
    Flush(oneshot::Sender<()>),
}

/// Sends each closed window that holds a call or a drop to Rail Center as its document, oldest
/// first, one at a time, retrying with a backoff from 5 s to 320 s; a document Rail Center refuses
/// is dropped. Besides the oldest, it keeps up to the newest window's plan's `max_unsent_windows`
/// and up to two million records and steps, the oldest dropped past either. Each drop is counted in
/// the next document. Dropping it stops the sending.
pub struct CallRecordReporter {
    // Its task ends once this is dropped.
    messages: mpsc::UnboundedSender<Message>,
}

impl CallRecordReporter {
    /// Sends to `sink` on `runtime`, each document stamped by `now`; `jitter` scales every wait.
    pub fn start(
        runtime: &Handle,
        sink: impl CallRecordSink,
        now: WallClock,
        jitter: Jitter,
    ) -> Self {
        let (messages, received) = mpsc::unbounded_channel();
        runtime.spawn(run(sink, now, jitter, received));
        Self { messages }
    }

    /// Queues `window`, never waiting; an empty one isn't sent, nor counted as dropped.
    pub fn report(&self, window: ClosedWindow) {
        if window.is_empty() {
            tracing::debug!("a call record window with no call or drop, not sent");
            return;
        }
        let _ = self.messages.send(Message::Window(Box::new(window)));
    }

    /// Sends what's queued at once, not waiting out a backoff: once the plan is gone (a 204), no
    /// later window comes to send it.
    pub fn send_now(&self) {
        let _ = self.messages.send(Message::SendNow);
    }

    /// Sends what's queued, each once, not waiting out a backoff, for up to 2 s; returns once all
    /// were tried, or the time is out.
    pub async fn flush(&self) {
        let (done, flushed) = oneshot::channel();
        if self.messages.send(Message::Flush(done)).is_ok() {
            let _ = flushed.await;
        }
    }
}

// A window queued, made a document at its first attempt and sent as those bytes from then on.
struct Queued {
    content: Content,
    records_and_steps: usize,
    // The drops written into its document, handed on if it's dropped too; 0 until written.
    windows_dropped: u64,
}

enum Content {
    Window(Box<ClosedWindow>),
    Document { id: Uuid, body: Vec<u8> },
}

// What one attempt brought.
enum Delivery {
    Taken,
    // Rail Center won't take it: never sent again.
    Refused(String),
    Failed(String),
}

// The shutdown flush: until when it runs, and who waits for it.
struct Flush {
    until: Instant,
    waiting: Vec<oneshot::Sender<()>>,
}

// The task's state: it owns the queue, so nothing locks it.
struct Reporter {
    now: WallClock,
    backoff: Backoff,
    random: SystemRandom,
    queue: VecDeque<Queued>,
    // Drops not yet written into a document.
    windows_dropped: u64,
    next_attempt: Instant,
    // Whether the attempt after the one in flight skips the backoff.
    send_now: bool,
    // Each document is tried once while it runs.
    flush: Option<Flush>,
}

type Sending<'s> = Pin<Box<dyn Future<Output = Result<StatusCode, FetchError>> + Send + 's>>;

async fn run(
    sink: impl CallRecordSink,
    now: WallClock,
    jitter: Jitter,
    mut messages: mpsc::UnboundedReceiver<Message>,
) {
    let mut reporter = Reporter {
        now,
        backoff: Backoff::new(jitter),
        random: SystemRandom::new(),
        queue: VecDeque::new(),
        windows_dropped: 0,
        next_attempt: Instant::now(),
        send_now: false,
        flush: None,
    };
    let mut in_flight: Option<Sending> = None;
    loop {
        if in_flight.is_none() && reporter.queue.is_empty() {
            reporter.end_flush();
        }
        let due = in_flight.is_none() && !reporter.queue.is_empty();
        let flush_ends = reporter.flush.as_ref().map(|flush| flush.until);
        tokio::select! {
            message = messages.recv() => match message {
                None => return,
                Some(Message::Window(window)) => reporter.push(window, in_flight.is_some()),
                Some(Message::SendNow) => reporter.hurry(),
                Some(Message::Flush(done)) => reporter.start_flush(done),
            },
            answer = async { in_flight.as_mut().unwrap().await }, if in_flight.is_some() => {
                in_flight = None;
                reporter.answered(read(answer));
            }
            () = sleep_until(reporter.next_attempt), if due => {
                in_flight = reporter.attempt().map(|body| Box::pin(sink.send(body)) as Sending);
            }
            () = sleep_until(flush_ends.unwrap_or_else(Instant::now)), if flush_ends.is_some() => {
                reporter.end_flush();
            }
        }
    }
}

impl Reporter {
    fn push(&mut self, window: Box<ClosedWindow>, sending: bool) {
        // The newest window's plan is the one Rail Center last served, so its limit applies.
        let limit = window.max_unsent_windows() as usize;
        self.queue.push_back(Queued {
            records_and_steps: window.records_and_steps(),
            windows_dropped: 0,
            content: Content::Window(window),
        });
        self.trim(limit, sending);
    }

    // Past `limit` waiting besides the first, being sent or next, or past the records and steps
    // cap, the oldest goes.
    fn trim(&mut self, limit: usize, sending: bool) {
        let mut records_and_steps: usize = self.queue.iter().map(|q| q.records_and_steps).sum();
        // The one in flight stays.
        let first = usize::from(sending);
        loop {
            let too_many = self.queue.len() > limit + 1;
            if !too_many && records_and_steps <= MAX_QUEUED_RECORDS_AND_STEPS {
                return;
            }
            if first >= self.queue.len() {
                return;
            }
            let dropped = self.queue.remove(first).expect("an index in the queue");
            records_and_steps -= dropped.records_and_steps;
            self.count_drop(dropped.windows_dropped, "the queue is past its limits");
        }
    }

    // Counted in the next document, with the drops it carried.
    fn count_drop(&mut self, carried: u64, why: &str) {
        self.windows_dropped += carried + 1;
        tracing::warn!(
            why,
            queued = self.queue.len(),
            windows_dropped = self.windows_dropped,
            "dropping a call record window; Rail Center reads it as missing"
        );
    }

    // The next attempt goes now; one in flight, if failing, is retried at once.
    fn hurry(&mut self) {
        self.send_now = true;
        self.next_attempt = Instant::now();
    }

    fn start_flush(&mut self, done: oneshot::Sender<()>) {
        let until = Instant::now() + FLUSH_LIMIT;
        let flush = self.flush.get_or_insert_with(|| Flush {
            until,
            waiting: Vec::new(),
        });
        flush.waiting.push(done);
        self.next_attempt = Instant::now();
    }

    fn end_flush(&mut self) {
        let Some(flush) = self.flush.take() else {
            return;
        };
        if !self.queue.is_empty() {
            tracing::warn!(
                unsent = self.queue.len(),
                "call record windows left unsent at shutdown"
            );
        }
        for done in flush.waiting {
            let _ = done.send(());
        }
    }

    // The oldest's bytes, made its document if it isn't yet; `None` if it can't be.
    fn attempt(&mut self) -> Option<Vec<u8>> {
        // A hurry is for the attempt in flight, or this one: not for its retries.
        self.send_now = false;
        if let Content::Document { body, .. } = &self.queue.front()?.content {
            return Some(body.clone());
        }
        let mut random = [0; 16];
        if self.random.fill(&mut random).is_err() {
            self.answered(Delivery::Failed("no random bytes for its id".to_owned()));
            return None;
        }
        let id = uuid::Builder::from_random_bytes(random).into_uuid();
        let mut head = self.queue.pop_front()?;
        let Content::Window(window) = head.content else {
            unreachable!("the oldest was a window just above")
        };
        let document = window.into_document(id, (self.now)(), self.windows_dropped);
        match serde_json::to_vec(&document) {
            Ok(body) => {
                head.content = Content::Document {
                    id,
                    body: body.clone(),
                };
                head.windows_dropped = mem::take(&mut self.windows_dropped);
                self.queue.push_front(head);
                Some(body)
            }
            // A clock outside years 0 to 9999: the document can't write its times.
            Err(error) => {
                tracing::error!(%error, document_id = %id, "a call record document can't be written");
                self.count_drop(0, "it can't be written");
                self.next_attempt = Instant::now();
                None
            }
        }
    }

    fn answered(&mut self, delivery: Delivery) {
        let document_id = match self.queue.front().map(|head| &head.content) {
            Some(Content::Document { id, .. }) => Some(id.to_string()),
            _ => None,
        };
        let hurry = mem::take(&mut self.send_now);
        let dropped = match delivery {
            Delivery::Failed(reason) if self.flush.is_none() => {
                // A failure counts even when hurried: only a document taken ends the run.
                self.backoff.count_failure();
                let wait = if hurry {
                    Duration::ZERO
                } else {
                    self.backoff.wait()
                };
                self.next_attempt = Instant::now() + wait;
                tracing::warn!(
                    reason,
                    document_id,
                    queued = self.queue.len(),
                    retry = round_to_seconds(wait),
                    "call records send failed; keeping them to send again"
                );
                return;
            }
            Delivery::Taken => {
                self.backoff.reset();
                tracing::debug!(document_id, "call records sent");
                None
            }
            Delivery::Refused(reason) => {
                tracing::error!(reason, document_id, "Rail Center refused call records");
                Some("Rail Center refused it")
            }
            // Each is tried once while flushing: the process is stopping.
            Delivery::Failed(reason) => {
                tracing::warn!(reason, document_id, "call records send failed at shutdown");
                Some("it failed at shutdown")
            }
        };
        let head = self.queue.pop_front().expect("the document answered");
        self.next_attempt = Instant::now();
        if let Some(why) = dropped {
            self.count_drop(head.windows_dropped, why);
        }
    }
}

// Any 2xx is taken; any other 4xx but a refused credential, 408 and 429 is Rail Center refusing
// the document, which a retry can't change; everything else is retried.
fn read(answer: Result<StatusCode, FetchError>) -> Delivery {
    let status = match answer {
        Ok(status) => status,
        Err(error) => return Delivery::Failed(error.to_string()),
    };
    if status.is_success() {
        return Delivery::Taken;
    }
    let retried = matches!(
        status,
        StatusCode::UNAUTHORIZED
            | StatusCode::FORBIDDEN
            | StatusCode::REQUEST_TIMEOUT
            | StatusCode::TOO_MANY_REQUESTS
    );
    if status.is_client_error() && !retried {
        Delivery::Refused(answered(status))
    } else {
        Delivery::Failed(answered(status))
    }
}
