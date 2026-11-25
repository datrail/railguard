//! Capture: the calls the callouts hand over, counted in windows under the plan held, on a thread of
//! its own, each closed window handed to the reporter.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use rail_model::capture::{
    BodyUnreadReason, CallOutcome, ClosedWindow, CompiledPlan, ObservedCall, Windowing,
};
use tokio::runtime::Handle;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::sleep;

use crate::call_record::CallRecordReporter;
use crate::capture_plan_holder::CapturePlanHolder;
use crate::clock::WallClock;
use crate::runtime_thread::RuntimeThread;

// Calls waiting for the capture task: past it, a call is dropped and counted.
const QUEUE: usize = 4_096;

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
    calls: mpsc::Sender<ObservedCall>,
    // Calls the full queue turned away, read into the open window.
    dropped: Arc<AtomicU64>,
}

impl Capture {
    /// Starts the task on `runtime`, telling times by `now`.
    pub fn start(
        runtime: &Handle,
        plans: CapturePlanHolder,
        reporter: CallRecordReporter,
        now: WallClock,
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
            now,
        };
        let task = runtime.spawn(task.run(queued, stopped));
        let sender = CaptureSender {
            plans,
            calls,
            dropped,
        };
        (Self { stop, task }, sender)
    }

    /// Counts the calls already handed over, closes the open window and sends the windows waiting,
    /// for up to 2 s.
    pub async fn stop(self) {
        let _ = self.stop.send(());
        // Cancelled by its runtime stopping first: what it held isn't sent.
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
    pub fn send(&self, call: ObservedCall) {
        if let Err(TrySendError::Full(_)) = self.calls.try_send(call) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
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
    now: WallClock,
}

impl Task {
    async fn run(
        mut self,
        mut queued: mpsc::Receiver<ObservedCall>,
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
                    while let Ok(call) = queued.try_recv() {
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

    // The plan held now: its change or going, the window's time over, or the clock stepped back
    // closes the window.
    fn hold(&mut self) {
        let (plan, now) = self.read();
        let gone = plan.is_none();
        let closed = self.windowing.hold(plan, now);
        self.hand_over(closed, gone);
    }

    fn record(&mut self, call: &ObservedCall) {
        let (plan, now) = self.read();
        let gone = plan.is_none();
        // A call that panics its reading is dropped, so one odd call can't stop capture for good.
        let windowing = &mut self.windowing;
        let Ok(recorded) = catch_unwind(AssertUnwindSafe(|| windowing.record(plan, call, now)))
        else {
            // Not the panic's message, which may quote the call.
            tracing::error!("recording a call panicked; dropping it");
            return;
        };
        self.hand_over(recorded.closed, gone);
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
        if dropped > 0 && self.windowing.end().is_none() {
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
