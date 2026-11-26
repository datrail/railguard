//! Capture: the calls the callouts hand over, counted in windows under the plan held, on a thread of
//! its own, each closed window handed to the reporter.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use rail_model::body::ByteBudget;
use rail_model::capture::{BodyUnreadReason, CallOutcome, ClosedWindow, CompiledPlan, Windowing};
use rail_model::request::ObservedRequest;
use tokio::runtime::Handle;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::sleep;

use crate::call_record::CallRecordReporter;
use crate::capture_plan_holder::CapturePlanHolder;
use crate::clock::WallClock;
use crate::gather::{BodyEnding, HeaderLines, RequestGatherer};
use crate::runtime_thread::RuntimeThread;

// Calls waiting for the capture task: past it, a call is dropped and counted.
const QUEUE: usize = 4_096;

// What every body kept for capture may take together.
const BODY_BUDGET_BYTES: usize = 256 * 1024 * 1024;

// The longest the task sleeps with a window open, a minute, the shortest window: a clock stepped
// back is seen though no call comes.
const MAX_SLEEP: Duration = Duration::from_secs(60);

/// The thread capture runs on, on a runtime of its own, so its matching and grouping delay neither
/// the callouts nor Rail Center's tasks.
pub struct CaptureThread(RuntimeThread);

impl CaptureThread {
    pub fn start() -> Result<Self, String> {
        RuntimeThread::start("capture", "capture").map(Self)
    }

    pub fn runtime(&self) -> &Handle {
        self.0.runtime()
    }

    /// Stops the thread, dropping every task still on it.
    pub fn stop(self) {
        self.0.stop();
    }
}

/// The capture task: it counts each call handed over in the window open under the plan `plans`
/// holds, and hands each window closed to `reporter`, sent at once when the plan goes. Dropping it
/// stops it as `stop` does, unawaited.
pub struct Capture {
    stop: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

/// What the callouts hand calls over through, never waiting.
#[derive(Clone)]
pub struct CaptureSender {
    plans: Arc<CapturePlanHolder>,
    calls: mpsc::Sender<ObservedRequest>,
    // Calls the full queue turned away, read into the open window.
    dropped: Arc<AtomicU64>,
    // Past it, a body is handed over unread.
    max_body_bytes: usize,
    budget: Arc<ByteBudget>,
}

/// A call held for capture from its headers until its host sees its body end; dropped first, it's
/// handed over as its stream ending first: `BodyEnding::CutShort`.
pub struct HeldRequest {
    // Taken when handed over.
    gatherer: Option<RequestGatherer>,
    sender: CaptureSender,
}

impl Capture {
    /// Starts the task on `runtime`, telling times by `now`; the bodies past `max_body_bytes`, or
    /// past the 256 MiB every body kept shares, are handed over unread.
    pub fn start(
        runtime: &Handle,
        plans: CapturePlanHolder,
        reporter: CallRecordReporter,
        now: WallClock,
        max_body_bytes: usize,
    ) -> (Self, CaptureSender) {
        let (calls, queued) = mpsc::channel(QUEUE);
        let (stop, stopped) = oneshot::channel();
        let plans = Arc::new(plans);
        let dropped = Arc::new(AtomicU64::new(0));
        let task = Task {
            windowing: Windowing::default(),
            plans: Arc::clone(&plans),
            reporter,
            dropped: Arc::clone(&dropped),
            said: Vec::new(),
            panicked: false,
            now,
        };
        let task = runtime.spawn(task.run(queued, stopped));
        let sender = CaptureSender {
            plans,
            calls,
            dropped,
            max_body_bytes,
            budget: ByteBudget::new(BODY_BUDGET_BYTES),
        };
        (Self { stop, task }, sender)
    }

    /// Counts the calls already handed over (at most 4096 waiting, untimed), closes the open window
    /// and sends the windows waiting, waiting up to 2 s for them.
    pub async fn stop(self) {
        let _ = self.stop.send(());
        // Cancelled by its runtime stopping first, or panicked: what it held isn't sent.
        if self.task.await.is_err() {
            tracing::warn!("capture stopped before it could close its window and send it");
        }
    }
}

impl CaptureSender {
    /// Whether a plan is held and the task takes calls: read at a call's headers, it decides that
    /// call's capture.
    pub fn is_capturing(&self) -> bool {
        !self.calls.is_closed() && self.plans.snapshot().is_some()
    }

    /// Hands `call` over; one the full queue turns away is counted as dropped.
    pub fn send(&self, call: ObservedRequest) {
        if let Err(TrySendError::Full(_)) = self.calls.try_send(call) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// The call whose header lines are `lines`, held until its body ends; `None` unless capturing.
    pub fn hold<N, V>(&self, lines: impl IntoIterator<Item = (N, V)>) -> Option<HeldRequest>
    where
        N: AsRef<[u8]>,
        V: AsRef<[u8]> + Into<Vec<u8>>,
    {
        self.is_capturing().then(|| HeldRequest {
            gatherer: Some(RequestGatherer::start(
                lines,
                HeaderLines::Dropped,
                self.max_body_bytes,
                &self.budget,
            )),
            sender: self.clone(),
        })
    }
}

impl HeldRequest {
    /// Keeps `body`, sent in one message.
    pub fn adopt(&mut self, body: Vec<u8>) {
        if let Some(gatherer) = &mut self.gatherer {
            gatherer.adopt(body);
        }
    }

    /// Hands the call over, its body as `ending` tells.
    pub fn finish(mut self, ending: BodyEnding) {
        if let Some(gatherer) = self.gatherer.take() {
            self.sender.send(gatherer.finish(ending));
        }
    }
}

impl Drop for HeldRequest {
    fn drop(&mut self) {
        if let Some(gatherer) = self.gatherer.take() {
            self.sender.send(gatherer.finish(BodyEnding::CutShort));
        }
    }
}

// The task's state: it owns the windowing, so nothing locks it.
struct Task {
    windowing: Windowing,
    plans: Arc<CapturePlanHolder>,
    reporter: CallRecordReporter,
    dropped: Arc<AtomicU64>,
    // The body-unread reasons logged at `warn` so far.
    said: Vec<BodyUnreadReason>,
    // Whether a call's recording panicked yet, logged at `error`.
    panicked: bool,
    now: WallClock,
}

impl Task {
    async fn run(
        mut self,
        mut queued: mpsc::Receiver<ObservedRequest>,
        mut stopped: oneshot::Receiver<()>,
    ) {
        let mut changes = self.plans.changes();
        // Each closed for good once its other side is gone, so neither is waited on again.
        let (mut plans_open, mut calls_open) = (true, true);
        self.hold();
        loop {
            let wake = self.windowing.end().map(|end| {
                let wait = end.duration_since((self.now)()).unwrap_or_default();
                wait.min(MAX_SLEEP)
            });
            tokio::select! {
                // A stop first, then a plan change before the calls taken under it.
                biased;
                // Stopped, or `Capture` dropped: the calls handed over are counted, not lost.
                _ = &mut stopped => {
                    // Closed first, so calls still coming can't keep the drain going.
                    queued.close();
                    // After `close`, `recv` waits only for sends already under way.
                    while let Some(call) = queued.recv().await {
                        self.record(&call);
                    }
                    let (_, now) = self.read();
                    let closed = self.windowing.hold(None, now);
                    self.hand_over(closed, false);
                    self.reporter.flush().await;
                    return;
                }
                changed = changes.changed(), if plans_open => match changed {
                    Ok(()) => self.hold(),
                    Err(_) => plans_open = false,
                },
                call = queued.recv(), if calls_open => match call {
                    Some(call) => self.record(&call),
                    None => calls_open = false,
                },
                () = sleep(wake.unwrap_or_default()), if wake.is_some() => self.hold(),
            }
        }
    }

    // The plan held now: its change or going, the window's time over, or the clock stepped back more
    // than a minute closes the window.
    fn hold(&mut self) {
        let (plan, now) = self.read();
        let gone = plan.is_none();
        let closed = self.windowing.hold(plan, now);
        self.hand_over(closed, gone);
    }

    fn record(&mut self, call: &ObservedRequest) {
        let (plan, now) = self.read();
        let gone = plan.is_none();
        // The window the call's time closes handed over first, so a panic below can't lose it.
        let closed = self.windowing.hold(plan.clone(), now);
        self.hand_over(closed, gone);
        // A call whose recording panics is dropped, so one odd call can't stop capture for good.
        let windowing = &mut self.windowing;
        let Ok(recorded) = catch_unwind(AssertUnwindSafe(|| windowing.record(plan, call, now)))
        else {
            self.log_panicked();
            return;
        };
        match recorded.outcome {
            CallOutcome::Counted {
                body_unread: Some(reason),
            } => self.log_body_unread(reason),
            CallOutcome::Counted { body_unread: None } => {}
            CallOutcome::Dropped => {
                tracing::debug!("a call taken with no capture plan held, not counted");
            }
        }
    }

    // The queue's drops told to the window open first, so they count where they were lost; the
    // plan held and the time now.
    fn read(&mut self) -> (Option<Arc<CompiledPlan>>, SystemTime) {
        let dropped = self.dropped.swap(0, Ordering::Relaxed);
        if dropped > 0 && !self.windowing.is_open() {
            tracing::debug!(calls = dropped, "calls dropped with no capture window open");
        }
        self.windowing.count_dropped(dropped);
        (self.plans.snapshot(), (self.now)())
    }

    // Once the plan's gone, no later window comes to carry it, so it goes at once.
    fn hand_over(&self, closed: Option<ClosedWindow>, gone: bool) {
        let Some(closed) = closed else {
            return;
        };
        self.reporter.report(closed);
        if gone {
            self.reporter.send_now();
        }
    }

    // At `error` the first time, then at `debug`: one bug on a common path panics on every call.
    // Not the panic's message, which may quote the call.
    fn log_panicked(&mut self) {
        if self.panicked {
            tracing::debug!("recording a call panicked; dropping it");
        } else {
            self.panicked = true;
            tracing::error!("recording a call panicked; dropping it, and each other that does");
        }
    }

    // At `warn` the first time each reason is seen, each calling for its own fix; then at `debug`.
    fn log_body_unread(&mut self, reason: BodyUnreadReason) {
        let message =
            "a call an entry could have taken went to the catch-all, its body's values not told";
        if self.said.contains(&reason) {
            tracing::debug!(reason = reason.as_str(), message);
        } else {
            self.said.push(reason);
            tracing::warn!(reason = reason.as_str(), "{message}; said once per reason");
        }
    }
}
