//! Capture: the calls the callouts hand over, counted in windows under the plan held, on a thread of
//! its own, each closed window handed to the reporter.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use rail_model::capture::{BodyUnreadReason, CallOutcome, ClosedWindow, ObservedCall, Windowing};
use tokio::runtime::Handle;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, oneshot};
use tokio::time::sleep;

use crate::call_record::CallRecordReporter;
use crate::capture_plan_holder::CapturePlanHolder;
use crate::clock::WallClock;
use crate::runtime_thread::RuntimeThread;

// Calls waiting for the capture task: past it, a call is dropped and counted.
const QUEUE: usize = 4_096;

// The longest the task sleeps with a window open: the shortest window, so a clock stepped back is
// seen though no call comes.
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
/// holds, and hands each window closed to `reporter`, sent at once when the plan goes.
pub struct Capture {
    stop: oneshot::Sender<oneshot::Sender<()>>,
    reporter: Arc<CallRecordReporter>,
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
        let reporter = Arc::new(reporter);
        let dropped = Arc::new(AtomicU64::new(0));
        let task = Task {
            windowing: Windowing::default(),
            plans: Arc::clone(&plans),
            reporter: Arc::clone(&reporter),
            dropped: Arc::clone(&dropped),
            said: Vec::new(),
            now,
        };
        runtime.spawn(task.run(queued, stopped));
        let sender = CaptureSender {
            plans,
            calls,
            dropped,
        };
        (Self { stop, reporter }, sender)
    }

    /// Closes the open window and sends what's queued, for up to 2 s.
    pub async fn stop(self) {
        let (done, closed) = oneshot::channel();
        if self.stop.send(done).is_ok() {
            let _ = closed.await;
        }
        self.reporter.flush().await;
    }
}

impl CaptureSender {
    /// Whether a plan is held: read at a call's headers, it decides that call's capture.
    pub fn is_capturing(&self) -> bool {
        self.plans.snapshot().is_some()
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
    reporter: Arc<CallRecordReporter>,
    dropped: Arc<AtomicU64>,
    // The body-unread reasons logged at `warn` so far.
    said: Vec<BodyUnreadReason>,
    now: WallClock,
}

impl Task {
    async fn run(
        mut self,
        mut queued: mpsc::Receiver<ObservedCall>,
        mut stopped: oneshot::Receiver<oneshot::Sender<()>>,
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
                done = &mut stopped => {
                    self.close();
                    if let Ok(done) = done {
                        let _ = done.send(());
                    }
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

    // The plan held now: its change, its going, or the window's time over closes the window.
    fn hold(&mut self) {
        let now = self.read_dropped();
        let plan = self.plans.snapshot();
        let gone = plan.is_none();
        if let Some(closed) = self.windowing.hold(plan, now) {
            self.hand_over(closed, gone);
        }
    }

    fn record(&mut self, call: &ObservedCall) {
        let now = self.read_dropped();
        let plan = self.plans.snapshot();
        let gone = plan.is_none();
        let recorded = self.windowing.record(plan, call, now);
        if let Some(closed) = recorded.closed {
            self.hand_over(closed, gone);
        }
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

    // The window open closed for capture to stop, to send with what's queued.
    fn close(&mut self) {
        let now = self.read_dropped();
        if let Some(closed) = self.windowing.hold(None, now) {
            self.reporter.report(closed);
        }
    }

    // The queue's drops told to the window open before anything else; the time now.
    fn read_dropped(&mut self) -> SystemTime {
        self.windowing
            .count_dropped(self.dropped.swap(0, Ordering::Relaxed));
        (self.now)()
    }

    // Once the plan's gone, no later window comes to carry it, so it goes at once.
    fn hand_over(&self, closed: ClosedWindow, gone: bool) {
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
