use tokio::runtime::Handle;

use crate::runtime_thread::RuntimeThread;

/// The thread every Rail Center task runs on, on a runtime of its own, so a burst of Envoy calls
/// can't delay a refresh and a refresh's work can't stall a call.
pub struct RailCenterThread(RuntimeThread);

impl RailCenterThread {
    pub fn start() -> Result<Self, String> {
        RuntimeThread::start("rail-center", "Rail Center").map(Self)
    }

    pub fn runtime(&self) -> &Handle {
        self.0.runtime()
    }

    /// Stops the thread, dropping every task still on it.
    pub fn stop(self) {
        self.0.stop();
    }
}
