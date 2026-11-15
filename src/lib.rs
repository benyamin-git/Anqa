pub mod auth;
pub mod bots;
pub mod config;
pub mod docker;
pub mod healthcheck;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
