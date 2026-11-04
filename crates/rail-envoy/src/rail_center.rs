use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tokio::runtime::{Builder, Handle};
use tokio::sync::oneshot;

// A fetch still resolving a name when the process stops isn't waited for any longer.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(1);

/// The thread every Rail Center task runs on, on a runtime of its own, so a burst of Envoy calls
/// can't delay a refresh and a refresh's work can't stall a call.
pub struct RailCenterThread {
    runtime: Handle,
    stop: oneshot::Sender<()>,
    thread: JoinHandle<()>,
}

impl RailCenterThread {
    pub fn start() -> Result<Self, String> {
        let (stop, stopped) = oneshot::channel::<()>();
        let (started, handle) = mpsc::channel();
        // Built on its own thread, so it's never dropped inside the caller's runtime.
        let thread = thread::Builder::new()
            .name("rail-center".to_owned())
            .spawn(move || {
                let runtime = match Builder::new_current_thread().enable_all().build() {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = started.send(Err(error.to_string()));
                        return;
                    }
                };
                let _ = started.send(Ok(runtime.handle().clone()));
                // A current-thread runtime runs its tasks only while something blocks on it.
                let _ = runtime.block_on(stopped);
                runtime.shutdown_timeout(SHUTDOWN_GRACE);
            })
            .map_err(|error| format!("can't start the Rail Center thread: {error}"))?;
        let runtime = handle
            .recv()
            .map_err(|_| "the Rail Center thread stopped as it started".to_owned())?
            .map_err(|error| format!("can't start the Rail Center runtime: {error}"))?;
        Ok(Self {
            runtime,
            stop,
            thread,
        })
    }

    pub fn runtime(&self) -> &Handle {
        &self.runtime
    }

    /// Stops the thread, dropping every task still on it.
    pub fn stop(self) {
        let _ = self.stop.send(());
        if self.thread.join().is_err() {
            tracing::error!("the Rail Center thread panicked");
        }
    }
}
