//! Capture's windows over time: the one open under the plan held, each closed when its time ends,
//! the plan changes or goes, or capture stops, and the next opened then.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::call::ObservedCall;
use super::compile::CompiledPlan;
use super::window::{BodyUnreadReason, ClosedWindow, Window};

/// The open window, if a plan is held, and when it ends; told the time at each call, so a time
/// passed in any of them closes it.
#[derive(Debug, Default)]
pub struct Windowing {
    open: Option<Open>,
}

#[derive(Debug)]
struct Open {
    window: Window,
    end: SystemTime,
}

/// A call counted in the open window.
#[derive(Debug)]
pub struct Counted {
    /// The window that ended before the call, if one did.
    pub closed: Option<ClosedWindow>,
    pub body_unread: Option<BodyUnreadReason>,
}

impl Windowing {
    pub fn new() -> Self {
        Self::default()
    }

    /// When the open window ends, if one is: the time to call `advance` at.
    pub fn end(&self) -> Option<SystemTime> {
        self.open.as_ref().map(|open| open.end)
    }

    /// The plan held at `now`, told apart from the last by pointer: another closes the open window
    /// at `now`, or its end if sooner, and opens one under it; none closes it, the last until one
    /// is held.
    pub fn hold(
        &mut self,
        plan: Option<Arc<CompiledPlan>>,
        now: SystemTime,
    ) -> Option<ClosedWindow> {
        let unchanged = match (&self.open, &plan) {
            (Some(open), Some(plan)) => Arc::ptr_eq(open.window.plan(), plan),
            (None, None) => true,
            _ => false,
        };
        if unchanged {
            return self.advance(now);
        }
        let closed = self
            .open
            .take()
            .map(|open| open.window.close(now.min(open.end)));
        // A clock stepped back mustn't open a window over the one closed.
        let start = closed.as_ref().map_or(now, |closed| now.max(closed.end()));
        self.open = plan.map(|plan| Open::new(plan, start));
        closed
    }

    /// Closes the open window if it ended by `now`, and opens the next at `now`: a time not seen,
    /// the process suspended or the clock stepped forward, is no window's.
    pub fn advance(&mut self, now: SystemTime) -> Option<ClosedWindow> {
        let open = self.open.take_if(|open| now >= open.end)?;
        let plan = Arc::clone(open.window.plan());
        let closed = open.window.close(open.end);
        self.open = Some(Open::new(plan, now));
        Some(closed)
    }

    /// Counts `call`, received at `at`, in the window open then; `None`, the call dropped, if no
    /// plan is held.
    pub fn record(&mut self, call: &ObservedCall, at: SystemTime) -> Option<Counted> {
        let closed = self.advance(at);
        let open = self.open.as_mut()?;
        Some(Counted {
            closed,
            body_unread: open.window.record(call, at),
        })
    }

    /// Counts calls lost before reaching the open window; with none open, they're no window's.
    pub fn count_dropped(&mut self, calls: u64) {
        if let Some(open) = &mut self.open {
            open.window.count_dropped(calls);
        }
    }

    /// Closes the open window at `now`, or its end if sooner, for capture to stop.
    pub fn close(self, now: SystemTime) -> Option<ClosedWindow> {
        self.open.map(|open| open.window.close(now.min(open.end)))
    }
}

impl Open {
    // Ends at the first multiple of the plan's `window_seconds` since the epoch after `start`.
    fn new(plan: Arc<CompiledPlan>, start: SystemTime) -> Self {
        let seconds = u64::from(plan.plan().window_seconds);
        // The proxy never runs before 1970, so such a time reads as the epoch.
        let since_epoch = start
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let end = UNIX_EPOCH + Duration::from_secs((since_epoch / seconds + 1) * seconds);
        Self {
            window: Window::open(plan, start),
            end,
        }
    }
}
