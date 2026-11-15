pub mod config;
pub mod healthcheck;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
