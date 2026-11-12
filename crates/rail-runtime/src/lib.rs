//! The IO: configuration, the log, and talking to Rail Center.

mod body;
mod bundle;
mod bundle_holder;
mod client;
mod clock;
mod config;
mod denial;
mod gateway;
mod gateway_call;
mod holder;
mod key_set;
pub mod logging;
mod protected_hosts;
mod rail_center;
mod rail_center_thread;
mod ticket;
mod ticket_holder;
mod xrail;

pub use bundle::{BundleSettings, BundleSource, RailCenterBundles, read_bundle_settings};
pub use bundle_holder::{BundleHolder, HeldBundle, Posture};
pub use client::{FetchError, Limits, RailCenterClient, Response};
pub use clock::{Clock, Stamp};
pub use config::{
    ConfigErrors, EXIT_CODE, Entry, Level, Refused, Secret, Settings, Source, Startup, Variables,
};
pub use denial::{Denial, DenialReporter, DenialSink, DeniedBy, RailCenterDenials};
pub use gateway::{Answer, Call, UnitSettings};
pub use gateway_call::{Gateway, GatewaySettings};
pub use holder::Jitter;
pub use key_set::{KeySet, KeySetHolder, KeySource, RailCenterKeys};
pub use logging::{read_log_format, read_log_level};
pub use protected_hosts::{ProtectedHost, ProtectedHosts, read_protected_hosts};
pub use rail_center::{CenterUrl, RailCenterSettings, read_disabled, read_rail_center};
pub use rail_center_thread::RailCenterThread;
pub use ticket::{
    OpaqueTicket, RailCenterTickets, TicketAnswer, TicketQuery, TicketSettings, TicketSource,
    read_ticket_settings,
};
pub use ticket_holder::{TicketHolder, WallClock};
pub use xrail::{Route, Unavailable, XRAIL, XRailHeader, XRailHeaders, is_withheld_header};
