pub mod auth;
pub mod bots;
pub mod config;
pub mod docker;
pub mod healthcheck;

pub use docker::cleanup::{CleanupError, CleanupReport, ensure_orphan_cleanup};
pub use docker::lifecycle::{
    LifecycleError, container_name, set_autostart, set_restart_policy, start_bot, stop_bot,
    stop_container, volume_name,
};

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
