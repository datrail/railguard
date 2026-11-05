//! The IO: configuration, the log, and talking to Rail Center.

mod client;
mod config;
pub mod logging;
mod protected_hosts;
mod rail_center;
mod ticket;
mod ticket_holder;
mod xrail;

pub use client::{FetchError, Limits, RailCenterClient, Response};
pub use config::{
    ConfigErrors, EXIT_CODE, Entry, Level, Refused, Secret, Settings, Source, Startup, Variables,
};
pub use protected_hosts::{PROTECTED_HOSTS, ProtectedHost, ProtectedHosts, read_protected_hosts};
pub use rail_center::{
    ALLOW_HTTP, AUTH_MODE, AUTH_TOKEN, CENTER_URL, CenterUrl, DISABLED, RailCenterSettings,
    read_disabled, read_rail_center,
};
pub use ticket::{
    HOST_ID, OpaqueTicket, SANDBOX_NAME, TICKET_REFRESH_SECONDS, TicketAnswer, TicketQuery,
    TicketSettings, fetch_ticket, read_ticket_settings,
};
pub use ticket_holder::{RailCenterTickets, TicketHolder, TicketSource, Unavailable, WallClock};
pub use xrail::{Route, XRailHeader, XRailHeaders};
