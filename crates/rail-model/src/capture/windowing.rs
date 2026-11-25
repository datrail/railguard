//! Capture's windows over time: one open at a time, under the plan held, closed when its time ends,
//! the plan changes or goes, or capture stops; the next opened when it's next told the time, while a
//! plan is held. Its time never goes back, but in one reset, so neither do its windows.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::call::ObservedCall;
use super::compile::CompiledPlan;
use super::window::{BodyUnreadReason, ClosedWindow, Window};

// A clock stepped back by up to this behind the latest time written is read as that time, so
// windows never overlap; further back, it's read as it is, else nothing would close until the
// clock caught up.
const ABSORBED_STEP_BACK: Duration = Duration::from_secs(60);

/// The open window, if a plan is held; `hold` and `record` are each told the plan held and the
/// time, so whichever sees the window's time over closes it.
#[derive(Debug, Default)]
pub struct Windowing {
    open: Option<Window>,
    // The last closed window's end: the latest time written while none is open.
    closed_end: Option<SystemTime>,
}

/// What `record` made of a call.
#[must_use = "a window it closed is lost unless it's sent"]
#[derive(Debug)]
pub struct Recorded {
    /// The window closed before the call was taken, if one was.
    pub closed: Option<ClosedWindow>,
    pub outcome: CallOutcome,
}

/// What became of the call.
#[derive(Debug)]
pub enum CallOutcome {
    /// Counted in the open window; why its body's values weren't told, if an entry could have
    /// taken it.
    Counted {
        body_unread: Option<BodyUnreadReason>,
    },
    /// No plan is held.
    Dropped,
}

impl Windowing {
    /// When the open window ends, if one is: when to call `hold` next, if nothing comes sooner.
    pub fn end(&self) -> Option<SystemTime> {
        self.open.as_ref().map(end_of)
    }

    /// The plan held at `now`, a new `Arc` being a new plan. The open window closes if the plan
    /// changed or went, or its time is over; then, while a plan is held, the next opens. A clock up
    /// to a minute behind the latest time written reads as that time; one further back closes the
    /// open window at its last call and opens the next at `now`, overlapping it.
    #[must_use = "a window it closed is lost unless it's sent"]
    pub fn hold(
        &mut self,
        plan: Option<Arc<CompiledPlan>>,
        now: SystemTime,
    ) -> Option<ClosedWindow> {
        let at = self.read(now);
        self.hold_at(plan, at)
    }

    /// Counts `call`, received at `at` with `plan` held, in the window open then, if one is.
    pub fn record(
        &mut self,
        plan: Option<Arc<CompiledPlan>>,
        call: &ObservedCall,
        at: SystemTime,
    ) -> Recorded {
        let at = self.read(at);
        let closed = self.hold_at(plan, at);
        let outcome = match &mut self.open {
            Some(open) => CallOutcome::Counted {
                body_unread: open.record(call, at),
            },
            None => CallOutcome::Dropped,
        };
        Recorded { closed, outcome }
    }

    /// Counts calls lost before reaching the open window: told before each `hold` or `record`,
    /// they count in the window open until then; with none open, they're no window's.
    pub fn count_dropped(&mut self, calls: u64) {
        if let Some(open) = &mut self.open {
            open.count_dropped(calls);
        }
    }

    // The windowing's time for the clock reading `now`.
    fn read(&self, now: SystemTime) -> SystemTime {
        let latest = self.open.as_ref().map(Window::latest).or(self.closed_end);
        match latest {
            Some(latest) if latest > now && latest <= now + ABSORBED_STEP_BACK => latest,
            _ => now,
        }
    }

    // `at` is the windowing's time: before the latest time written only past a reset.
    fn hold_at(&mut self, plan: Option<Arc<CompiledPlan>>, at: SystemTime) -> Option<ClosedWindow> {
        let keep = self.open.as_ref().is_some_and(|open| {
            plan.as_ref()
                .is_some_and(|plan| Arc::ptr_eq(open.plan(), plan))
                && (open.latest()..end_of(open)).contains(&at)
        });
        if keep {
            return None;
        }
        let closed = self.open.take().map(|open| {
            let end = end_of(&open);
            open.close(at.min(end))
        });
        if let Some(closed) = &closed {
            self.closed_end = Some(closed.end());
        }
        self.open = plan.map(|plan| Window::open(plan, at));
        closed
    }
}

// The first multiple of its plan's `window_seconds` since the epoch after its start.
fn end_of(window: &Window) -> SystemTime {
    let seconds = u64::from(window.plan().plan().window_seconds);
    // The proxy never runs before 1970, so such a time reads as the epoch.
    let since_epoch = window
        .start()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    UNIX_EPOCH + Duration::from_secs((since_epoch / seconds + 1) * seconds)
}
