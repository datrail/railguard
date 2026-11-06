//! The IO: configuration, the log, and talking to Rail Center.

mod client;
mod config;
mod holder;
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
pub use holder::Jitter;
pub use protected_hosts::{ProtectedHost, ProtectedHosts, read_protected_hosts};
pub use rail_center::{CenterUrl, RailCenterSettings, read_disabled, read_rail_center};
pub use ticket::{
    OpaqueTicket, RailCenterTickets, TicketAnswer, TicketQuery, TicketSettings, TicketSource,
    read_ticket_settings,
};
pub use ticket_holder::{TicketHolder, WallClock};
pub use xrail::{Route, Unavailable, XRailHeader, XRailHeaders};
