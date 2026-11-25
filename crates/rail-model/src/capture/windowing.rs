//! Capture's windows over time: one open at a time, under the plan held, closed when its time ends,
//! the plan changes or goes, or capture stops; the next opened when it's next told the time, while a
//! plan is held.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::call::ObservedCall;
use super::compile::CompiledPlan;
use super::window::{BodyUnreadReason, ClosedWindow, Window};

/// The open window, if a plan is held, and when it ends; each method is told the plan held and the
/// time, so whichever sees the window's time over closes it.
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
    /// The window closed before the call was counted, if one was.
    pub closed: Option<ClosedWindow>,
    /// Why the call's body values weren't told, if an entry could have taken it.
    pub body_unread: Option<BodyUnreadReason>,
}

impl Windowing {
    /// When the open window ends, if one is: when to call `hold` next, if nothing comes sooner.
    pub fn end(&self) -> Option<SystemTime> {
        self.open.as_ref().map(|open| open.end)
    }

    /// The plan held at `now`, a new `Arc` being a new plan, as the holder hands them out. The open
    /// window closes at `now`, or its end if sooner, if the plan changed or went (a 204, or capture
    /// stopping), its time is over, or the clock stepped back before its start; then, while a plan
    /// is held, the next opens at `now`.
    pub fn hold(
        &mut self,
        plan: Option<Arc<CompiledPlan>>,
        now: SystemTime,
    ) -> Option<ClosedWindow> {
        let closed = match self.open.take() {
            Some(open) => {
                let unchanged = plan
                    .as_ref()
                    .is_some_and(|plan| Arc::ptr_eq(open.window.plan(), plan));
                if unchanged && (open.window.start()..open.end).contains(&now) {
                    self.open = Some(open);
                    return None;
                }
                // Past a step back, the window ends at its last call, so the next may overlap it.
                Some(open.window.close(now.min(open.end)))
            }
            None => None,
        };
        self.open = plan.map(|plan| Open::new(plan, now));
        closed
    }

    /// Counts `call`, received at `at` with `plan` held, in the window open then; `None`, the call
    /// dropped, if no plan is held.
    pub fn record(
        &mut self,
        plan: Option<Arc<CompiledPlan>>,
        call: &ObservedCall,
        at: SystemTime,
    ) -> Option<Counted> {
        let closed = self.hold(plan, at);
        let open = self.open.as_mut()?;
        Some(Counted {
            closed,
            body_unread: open.window.record(call, at),
        })
    }

    /// Counts calls lost before reaching the open window: told before each `hold` or `record`, so
    /// they count in the window open when they were lost; with none open, they're no window's.
    pub fn count_dropped(&mut self, calls: u64) {
        if let Some(open) = &mut self.open {
            open.window.count_dropped(calls);
        }
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
