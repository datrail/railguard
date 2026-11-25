use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tokio::runtime::{Builder, Handle};
use tokio::sync::oneshot;

// A fetch still resolving a name when the process stops isn't waited for any longer.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(1);

/// A runtime on a thread of its own, named `name`; `what` names it in errors.
pub(crate) struct RuntimeThread {
    runtime: Handle,
    stop: oneshot::Sender<()>,
    thread: JoinHandle<()>,
}

impl RuntimeThread {
    pub(crate) fn start(name: &str, what: &str) -> Result<Self, String> {
        let (stop, stopped) = oneshot::channel::<()>();
        let (started, handle) = mpsc::channel();
        // Built on its own thread, so it's never dropped inside the caller's runtime.
        let thread = thread::Builder::new()
            .name(name.to_owned())
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
            .map_err(|error| format!("can't start the {what} thread: {error}"))?;
        let runtime = handle
            .recv()
            .map_err(|_| format!("the {what} thread stopped as it started"))?
            .map_err(|error| format!("can't start the {what} runtime: {error}"))?;
        Ok(Self {
            runtime,
            stop,
            thread,
        })
    }

    pub(crate) fn runtime(&self) -> &Handle {
        &self.runtime
    }

    /// Stops the thread, dropping every task still on it.
    pub(crate) fn stop(self) {
        let _ = self.stop.send(());
        // Its own code can't panic, and a task panicking on its runtime doesn't unwind it.
        let _ = self.thread.join();
    }
}
