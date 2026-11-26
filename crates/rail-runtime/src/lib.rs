//! The IO: configuration, the log, and talking to Rail Center.

mod bundle;
mod bundle_holder;
mod call_record;
mod callout;
mod capture;
mod capture_plan;
mod capture_plan_holder;
mod client;
mod clock;
mod config;
mod denial;
mod gateway;
mod gateway_call;
mod gateway_start;
mod holder;
mod key_set;
pub mod logging;
mod peek;
mod protected_hosts;
mod proxy_start;
mod rail_center;
mod rail_center_thread;
mod retry;
mod runtime_thread;
mod ticket;
mod ticket_holder;
mod xrail;

pub use bundle::{BundleSettings, BundleSource, RailCenterBundles, read_bundle_settings};
pub use bundle_holder::{BundleHolder, HeldBundle, Posture};
pub use call_record::{CallRecordReporter, CallRecordSink, RailCenterCallRecords};
pub use callout::{
    GATEWAY_MAX_BODY_MB, HostLimit, read_callout_port, read_max_body_bytes, read_max_message_bytes,
};
pub use capture::{Capture, CaptureSender, CaptureThread, SentBody, SentRequest};
pub use capture_plan::{CapturePlanSource, RailCenterCapturePlans, read_capture_plan_refresh};
pub use capture_plan_holder::CapturePlanHolder;
pub use client::{FetchError, Limits, RailCenterClient, Response};
pub use clock::{Clock, Stamp, WallClock};
pub use config::{
    ConfigErrors, EXIT_CODE, Entry, Level, Refused, Secret, Settings, Source, Startup, Variables,
};
pub use denial::{Denial, DenialReporter, DenialSink, DeniedBy, RailCenterDenials};
pub use gateway::{Answer, Call, UnitSettings};
pub use gateway_call::{Gateway, GatewaySettings};
pub use gateway_start::start_gateway;
pub use key_set::{KeySet, KeySetHolder, KeySource, RailCenterKeys};
pub use logging::{read_log_format, read_log_level};
pub use protected_hosts::{ProtectedHost, ProtectedHosts, read_protected_hosts};
pub use proxy_start::{Proxy, ProxySettings, ProxyStop, read_proxy_settings, start_proxy};
pub use rail_center::{
    CenterUrl, RailCenterSettings, read_disabled, read_rail_center, read_rail_center_with,
};
pub use rail_center_thread::RailCenterThread;
pub use rail_model::request::{XRAIL, is_withheld_header, read_path};
pub use retry::Jitter;
pub use ticket::{
    OpaqueTicket, RailCenterTickets, TicketSettings, TicketSource, read_ticket_settings,
};
pub use ticket_holder::TicketHolder;
pub use xrail::{Route, Unavailable, XRailHeader, XRailHeaders, withheld_headers};
