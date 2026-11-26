//! The proxy role started for a host: its ticket, capture plan and call records on the Rail Center
//! thread, capture on its own.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::call_record::{CallRecordReporter, RailCenterCallRecords};
use crate::capture::{Capture, CaptureSender, CaptureThread};
use crate::capture_plan::{RailCenterCapturePlans, read_capture_plan_refresh};
use crate::capture_plan_holder::CapturePlanHolder;
use crate::client::RailCenterClient;
use crate::config::{Refused, Settings};
use crate::rail_center::RailCenterSettings;
use crate::rail_center_thread::RailCenterThread;
use crate::retry::Jitter;
use crate::ticket::{RailCenterTickets, TicketSettings, read_ticket_settings};
use crate::ticket_holder::TicketHolder;

/// What the proxy reads from Rail Center, beside its settings.
#[derive(Debug)]
pub struct ProxySettings {
    pub ticket: TicketSettings,
    /// How often the capture plan is fetched again, before jitter.
    pub capture_plan_refresh: Duration,
}

/// The ticket's settings and the plan's refresh, judged whether or not Rail Center is used, which
/// `required` says; `None` if the host or sandbox is missing.
pub fn read_proxy_settings(
    settings: &mut Settings,
    required: bool,
) -> Result<Option<ProxySettings>, Refused> {
    let ticket = read_ticket_settings(settings, required);
    let used = matches!(ticket, Ok(Some(_)));
    let capture_plan_refresh = read_capture_plan_refresh(settings, used);
    let (ticket, capture_plan_refresh) = (ticket?, capture_plan_refresh?);
    Ok(ticket.map(|ticket| ProxySettings {
        ticket,
        capture_plan_refresh,
    }))
}

/// The proxy running: what its callouts read, and what stops it.
pub struct Proxy {
    pub tickets: Arc<TicketHolder>,
    pub capture: CaptureSender,
    pub stop: ProxyStop,
}

/// Stops the proxy's capture and threads, once its server has drained.
pub struct ProxyStop {
    capture: Capture,
    capture_thread: CaptureThread,
    rail_center: RailCenterThread,
}

impl ProxyStop {
    /// Closes the open window and sends what's waiting, then stops both threads; it blocks, so it
    /// runs on a blocking thread.
    pub fn stop(self) {
        // The window's upload runs on Rail Center's thread, so that one stops last.
        self.capture_thread.runtime().block_on(self.capture.stop());
        self.capture_thread.stop();
        self.rail_center.stop();
    }
}

/// Starts the proxy's holders, reporter and capture, a body past `max_body_bytes` captured unread;
/// calls only read what's held and hand calls over.
pub fn start_proxy(
    (rail_center, proxy): (RailCenterSettings, ProxySettings),
    max_body_bytes: usize,
) -> Result<Proxy, String> {
    let client = RailCenterClient::new(&rail_center)?;
    let thread = RailCenterThread::start()?;
    let capture_thread = CaptureThread::start()?;
    let runtime = thread.runtime();
    let agent = proxy.ticket.agent;
    let tickets = TicketHolder::start(
        runtime,
        RailCenterTickets {
            client: client.clone(),
            agent: agent.clone(),
        },
        proxy.ticket.refresh_interval,
        Arc::new(SystemTime::now),
        Jitter::random(),
    );
    let plans = CapturePlanHolder::start(
        runtime,
        RailCenterCapturePlans {
            client: client.clone(),
            agent,
        },
        proxy.capture_plan_refresh,
        Jitter::random(),
    );
    let reporter = CallRecordReporter::start(
        runtime,
        RailCenterCallRecords { client },
        Arc::new(SystemTime::now),
        Jitter::random(),
    );
    let (capture, sender) = Capture::start(
        capture_thread.runtime(),
        plans,
        reporter,
        Arc::new(SystemTime::now),
        max_body_bytes,
    );
    Ok(Proxy {
        tickets: Arc::new(tickets),
        capture: sender,
        stop: ProxyStop {
            capture,
            capture_thread,
            rail_center: thread,
        },
    })
}
