//! The IO: configuration, and talking to Rail Center.

mod client;
mod config;
mod rail_center;
mod ticket;
mod ticket_holder;

pub use client::{FetchError, Limits, RailCenterClient, Response};
pub use config::{ConfigErrors, EXIT_CODE, Secret, Settings, Variables};
pub use rail_center::{
    ALLOW_HTTP, AUTH_MODE, AUTH_TOKEN, CENTER_URL, CenterUrl, DISABLED, RailCenterSettings,
    read_disabled, read_rail_center,
};
pub use ticket::{
    HOST_ID, SANDBOX_NAME, TICKET_REFRESH_SECONDS, Ticket, TicketAnswer, TicketQuery,
    TicketSettings, fetch_ticket, read_ticket_settings,
};
pub use ticket_holder::{RailCenterTickets, TicketHolder, TicketSource, Unavailable, WallClock};
