//! The IO: configuration, and talking to Rail Center.

mod client;
mod config;
mod rail_center;

pub use client::{FetchError, Limits, RailCenterClient, Response};
pub use config::{ConfigErrors, EXIT_CODE, Secret, Settings, Variables};
pub use rail_center::{
    ALLOW_HTTP, AUTH_MODE, AUTH_TOKEN, CENTER_URL, CenterUrl, DISABLED, RailCenter,
    RailCenterSettings, read_rail_center,
};
