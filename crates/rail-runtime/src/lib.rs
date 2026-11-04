//! The IO: configuration, and talking to Rail Center.

mod client;
mod config;
mod rail_center;
mod ticket;
mod ticket_holder;

pub use client::{FetchError, Limits, RailCenterClient, Response};
pub use config::{ConfigErrors, EXIT_CODE, Secret, Settings, Variables};
pub use rail_center::{
    ALLOW_HTTP, AUTH_MODE, AUTH_TOKEN, CENTER_URL, CenterUrl, DISABLED, RailCenter,
    RailCenterSettings, read_rail_center,
};
pub use ticket::{TICKET_LIMITS, Ticket, TicketAnswer, TicketQuery, fetch_ticket};
pub use ticket_holder::{RailCenterTickets, TicketHolder, TicketSource, Unavailable, WallClock};
