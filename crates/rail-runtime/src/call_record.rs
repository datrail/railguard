//! Call record documents, sent to Rail Center in the background, oldest first, one at a time.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use rail_model::capture::ClosedWindow;
use reqwest::StatusCode;
use ring::rand::{SecureRandom, SystemRandom};
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot};
use tokio::task::AbortHandle;
use tokio::time::{Instant, sleep_until};
use uuid::Uuid;

use crate::client::{FetchError, Limits, RailCenterClient, Response, answered};
use crate::holder::{Jitter, backoff, round_to_seconds};
use crate::ticket_holder::WallClock;

const ROUTE: &str = "/v1/call-records";

// A document is up to a few MiB, sent in the background; Rail Center answers a few bytes.
const LIMITS: Limits = Limits {
    deadline: Duration::from_secs(30),
    silence: Duration::from_secs(5),
    max_bytes: 64 * 1024,
};

// Bounds the queue's memory however a plan's ceilings combine: two windows at them, nearly.
const MAX_QUEUED_RECORDS_AND_STEPS: usize = 2_000_000;

// How long the shutdown flush may take.
const FLUSH_LIMIT: Duration = Duration::from_secs(2);

/// Where documents go: Rail Center, or a test's script.
pub trait CallRecordSink: Send + Sync + 'static {
    /// One attempt to deliver `body`, whatever the status: what each means is the reporter's.
    fn send(&self, body: Vec<u8>) -> impl Future<Output = Result<Response, FetchError>> + Send;
}

/// Documents sent to Rail Center.
pub struct RailCenterCallRecords {
    pub client: RailCenterClient,
}

impl CallRecordSink for RailCenterCallRecords {
    async fn send(&self, body: Vec<u8>) -> Result<Response, FetchError> {
        self.client.post(ROUTE, body, LIMITS).await
    }
}

// A sink with its future boxed, so the task holds any sink without being generic.
trait ErasedSink: Send + Sync {
    fn send(&self, body: Vec<u8>) -> Sending;
}

type Sending = Pin<Box<dyn Future<Output = Result<Response, FetchError>> + Send>>;

impl<S: CallRecordSink> ErasedSink for Arc<S> {
    fn send(&self, body: Vec<u8>) -> Sending {
        let sink = Arc::clone(self);
        Box::pin(async move { CallRecordSink::send(&*sink, body).await })
    }
}

enum Message {
    Window(Box<ClosedWindow>),
    SendNow,
    Flush(oneshot::Sender<()>),
}

/// Sends each closed window to Rail Center as its document, oldest first, one in flight, retrying
/// with a backoff from 5 s to 320 s. Past the newest window's plan's `max_unsent_windows`, or past
/// two million records and steps queued, the oldest empty window not in flight is dropped, else
/// the oldest, and counted in the next document. Dropping it stops the sending.
pub struct CallRecordReporter {
    messages: mpsc::UnboundedSender<Message>,
    task: AbortHandle,
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
        let task = runtime
            .spawn(run(Box::new(Arc::new(sink)), now, jitter, received))
            .abort_handle();
        Self { messages, task }
    }

    /// Queues `window`, never waiting.
    pub fn report(&self, window: ClosedWindow) {
        let _ = self.messages.send(Message::Window(Box::new(window)));
    }

    /// Sends what's queued at once, not waiting out a backoff: after a 204, no later window is
    /// coming to carry it.
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

impl Drop for CallRecordReporter {
    fn drop(&mut self) {
        self.task.abort();
    }
}

// A window queued, made a document at its first attempt and sent as those bytes from then on.
struct Queued {
    content: Content,
    records_and_steps: usize,
    empty: bool,
    max_unsent_windows: u32,
}

enum Content {
    Window(Box<ClosedWindow>),
    Document {
        id: Uuid,
        body: Vec<u8>,
        // The drops written into it, handed on if it's dropped too.
        windows_dropped: u64,
    },
}

// What one attempt brought.
enum Delivery {
    Taken,
    // Rail Center won't take it: never sent again.
    Refused(String),
    Failed(String),
}

// The one task: owns the queue, so nothing locks it.
struct Reporter {
    sink: Box<dyn ErasedSink>,
    now: WallClock,
    jitter: Jitter,
    random: SystemRandom,
    queue: VecDeque<Queued>,
    // Drops not yet written into a document.
    windows_dropped: u64,
    // Attempts in a row that failed.
    failures: u32,
    next_attempt: Instant,
    // Until when the shutdown flush runs, and who waits for it; each is tried once then.
    flush: Option<(Instant, oneshot::Sender<()>)>,
}

async fn run(
    sink: Box<dyn ErasedSink>,
    now: WallClock,
    jitter: Jitter,
    mut messages: mpsc::UnboundedReceiver<Message>,
) {
    let mut reporter = Reporter {
        sink,
        now,
        jitter,
        random: SystemRandom::new(),
        queue: VecDeque::new(),
        windows_dropped: 0,
        failures: 0,
        next_attempt: Instant::now(),
        flush: None,
    };
    let mut in_flight: Option<Sending> = None;
    loop {
        reporter.end_flush_if_done(in_flight.is_some());
        let due = in_flight.is_none() && !reporter.queue.is_empty();
        let flush_ends = reporter.flush.as_ref().map(|(until, _)| *until);
        tokio::select! {
            message = messages.recv() => match message {
                None => return,
                Some(Message::Window(window)) => reporter.push(window, in_flight.is_some()),
                Some(Message::SendNow) => reporter.next_attempt = Instant::now(),
                Some(Message::Flush(done)) => {
                    reporter.flush = Some((Instant::now() + FLUSH_LIMIT, done));
                    reporter.next_attempt = Instant::now();
                }
            },
            answer = async { in_flight.as_mut().unwrap().await }, if in_flight.is_some() => {
                in_flight = None;
                reporter.answered(read(answer));
            }
            () = sleep_until(reporter.next_attempt), if due => {
                in_flight = reporter.attempt();
            }
            () = sleep_until(flush_ends.unwrap_or_else(Instant::now)), if flush_ends.is_some() => {
                reporter.end_flush();
            }
        }
    }
}

impl Reporter {
    fn push(&mut self, window: Box<ClosedWindow>, sending: bool) {
        self.queue.push_back(Queued {
            records_and_steps: window.records_and_steps(),
            empty: window.is_empty(),
            max_unsent_windows: window.max_unsent_windows(),
            content: Content::Window(window),
        });
        self.trim(sending);
    }

    // The newest window's plan is the one Rail Center last served, so its limit applies.
    fn trim(&mut self, sending: bool) {
        let limit = self
            .queue
            .back()
            .map_or(0, |queued| queued.max_unsent_windows as usize);
        // The one in flight stays.
        let first = usize::from(sending);
        loop {
            let records_and_steps: usize = self.queue.iter().map(|q| q.records_and_steps).sum();
            if self.queue.len() <= limit && records_and_steps <= MAX_QUEUED_RECORDS_AND_STEPS {
                return;
            }
            let Some(at) = (first..self.queue.len())
                .find(|&at| self.queue[at].empty)
                .or((first < self.queue.len()).then_some(first))
            else {
                return;
            };
            let dropped = self.queue.remove(at).expect("an index in the queue");
            self.drop_window(&dropped, "the queue is past its limits");
        }
    }

    // Counted in the next document, with the drops it carried.
    fn drop_window(&mut self, dropped: &Queued, why: &str) {
        let carried = match &dropped.content {
            Content::Window(_) => 0,
            Content::Document {
                windows_dropped, ..
            } => *windows_dropped,
        };
        self.windows_dropped += carried + 1;
        tracing::warn!(
            why,
            empty = dropped.empty,
            queued = self.queue.len(),
            windows_dropped = self.windows_dropped,
            "dropping a call record window; Rail Center reads it as missing"
        );
    }

    // Sends the oldest, made a document if it isn't yet; `None` if it can't be.
    fn attempt(&mut self) -> Option<Sending> {
        if let Content::Window(_) = self.queue.front()?.content {
            let mut random = [0; 16];
            if self.random.fill(&mut random).is_err() {
                self.answered(Delivery::Failed("no random bytes for its id".to_owned()));
                return None;
            }
            let id = uuid::Builder::from_random_bytes(random).into_uuid();
            self.write(id)?;
        }
        let Content::Document { body, .. } = &self.queue.front()?.content else {
            unreachable!("the oldest was just written")
        };
        Some(self.sink.send(body.clone()))
    }

    // Makes the oldest window its document, carrying the drops not yet written; `None` if a time
    // in it can't be written, which drops it.
    fn write(&mut self, id: Uuid) -> Option<()> {
        let mut head = self.queue.pop_front()?;
        let Content::Window(window) = head.content else {
            unreachable!("only a window is written")
        };
        let document = window.into_document(id, (self.now)(), self.windows_dropped);
        match serde_json::to_vec(&document) {
            Ok(body) => {
                head.content = Content::Document {
                    id,
                    body,
                    windows_dropped: self.windows_dropped,
                };
                self.windows_dropped = 0;
                self.queue.push_front(head);
                Some(())
            }
            Err(error) => {
                tracing::error!(%error, document_id = %id, "a call record document can't be written");
                self.windows_dropped += 1;
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
        let flushing = self.flush.is_some();
        match delivery {
            Delivery::Taken => {
                self.queue.pop_front();
                self.failures = 0;
                self.next_attempt = Instant::now();
                tracing::debug!(document_id, "call records sent");
            }
            Delivery::Refused(reason) => {
                let refused = self.queue.pop_front().expect("the head");
                self.failures = 0;
                self.next_attempt = Instant::now();
                tracing::error!(reason, document_id, "Rail Center refused call records");
                self.drop_window(&refused, "Rail Center refused it");
            }
            // Each is tried once while flushing: the process is stopping.
            Delivery::Failed(reason) if flushing => {
                let failed = self.queue.pop_front().expect("the head");
                tracing::warn!(reason, document_id, "call records send failed at shutdown");
                self.drop_window(&failed, "it failed at shutdown");
            }
            Delivery::Failed(reason) => {
                self.failures += 1;
                let wait = backoff(self.failures).mul_f64(self.jitter.factor());
                self.next_attempt = Instant::now() + wait;
                tracing::warn!(
                    reason,
                    document_id,
                    queued = self.queue.len(),
                    retry = round_to_seconds(wait),
                    "call records send failed; keeping them to send again"
                );
            }
        }
    }

    fn end_flush_if_done(&mut self, sending: bool) {
        if self.flush.is_some() && self.queue.is_empty() && !sending {
            self.end_flush();
        }
    }

    fn end_flush(&mut self) {
        if let Some((_, done)) = self.flush.take() {
            if !self.queue.is_empty() {
                tracing::warn!(
                    unsent = self.queue.len(),
                    "call record windows left unsent at shutdown"
                );
            }
            let _ = done.send(());
        }
    }
}

// Any 2xx is sent; any other 4xx but a refused credential, 408 and 429 is Rail Center refusing the
// document, which a retry can't change; everything else is retried.
fn read(answer: Result<Response, FetchError>) -> Delivery {
    let status = match answer {
        Ok(response) => response.status,
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
