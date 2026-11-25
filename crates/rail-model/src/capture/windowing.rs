//! Capture's windows over time: one open at a time, under the plan held, closed when its time ends,
//! the plan changes or goes, the clock steps back more than a minute, or capture stops; the next
//! opened when it's next told the time, while a plan is held. Its time, and so its windows, go back
//! only when the clock steps back more than a minute.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::call::ObservedCall;
use super::compile::CompiledPlan;
use super::window::{BodyUnreadReason, ClosedWindow, Window};

// A clock stepped back by up to this behind the latest time written is read as that time, so
// windows never overlap; further back, it's read as it is, else the open window would stay open
// until the clock caught up.
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
    /// When the open window ends, if one is and its end is a time: when to call `hold` next, if
    /// nothing comes sooner.
    pub fn end(&self) -> Option<SystemTime> {
        self.open.as_ref().and_then(end_of)
    }

    /// The plan held at `now`, a new `Arc` being a new plan. The open window closes if the plan
    /// changed or went, or its time is over, at the time read or its end if sooner; then, while a
    /// plan is held, the next opens at the time read. A clock up to `ABSORBED_STEP_BACK` (60 s)
    /// behind the latest time written reads as that time; one further back reads as it is, closing
    /// the open window at its latest time written and opening the next then, overlapping it.
    #[must_use = "a window it closed is lost unless it's sent"]
    pub fn hold(
        &mut self,
        plan: Option<Arc<CompiledPlan>>,
        now: SystemTime,
    ) -> Option<ClosedWindow> {
        let at = self.read(now);
        self.hold_at(plan, at)
    }

    /// Holds `plan` at `at` as `hold` does, then counts `call` in the window open, if one is,
    /// stamped at the time read.
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
            Some(latest)
                if now
                    .checked_add(ABSORBED_STEP_BACK)
                    .is_some_and(|limit| (now..=limit).contains(&latest)) =>
            {
                latest
            }
            _ => now,
        }
    }

    // `at` is the windowing's time: before the latest time written only past a reset.
    fn hold_at(&mut self, plan: Option<Arc<CompiledPlan>>, at: SystemTime) -> Option<ClosedWindow> {
        let keep = self.open.as_ref().is_some_and(|open| {
            plan.as_ref()
                .is_some_and(|plan| Arc::ptr_eq(open.plan(), plan))
                && open.latest() <= at
                && end_of(open).is_none_or(|end| at < end)
        });
        if keep {
            return None;
        }
        let closed = self.open.take().map(|open| {
            let end = end_of(&open).map_or(at, |end| at.min(end));
            open.close(end)
        });
        if let Some(closed) = &closed {
            self.closed_end = Some(closed.end());
        }
        self.open = plan.map(|plan| Window::open(plan, at));
        closed
    }
}

// The first multiple of its plan's `window_seconds` since the epoch after its start; none past
// the latest time, so a clock that far ends a window only by the plan going or stepping back.
fn end_of(window: &Window) -> Option<SystemTime> {
    let seconds = u64::from(window.plan().plan().window_seconds);
    // The proxy never runs before 1970, so such a time reads as the epoch.
    let since_epoch = window
        .start()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let end = (since_epoch / seconds + 1).checked_mul(seconds)?;
    UNIX_EPOCH.checked_add(Duration::from_secs(end))
}
