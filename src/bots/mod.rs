pub mod discovery;
pub mod manifest;
pub mod slug;

pub use discovery::{BotErrorCard, Discovery, ScanError, scan};
pub use manifest::load_bot;
pub use slug::slugify;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub name: Option<String>,
    pub image: Option<String>,
    pub env_file: Option<String>,
    pub command: Option<Vec<String>>,
    #[serde(default)]
    pub ports: Vec<String>,
    #[serde(default)]
    pub volumes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BotSource {
    Dockerfile,
    Image,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PortProtocol {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortSpec {
    pub host_ip: Option<String>,
    pub host_port: u16,
    pub container_port: u16,
    pub protocol: PortProtocol,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeSpec {
    pub name: String,
    pub target: PathBuf,
}

pub struct ResolvedBot {
    pub dir: String,
    pub slug: String,
    pub name: String,
    pub image: String,
    pub source: BotSource,
    pub manifest: Manifest,
    pub env: Vec<(String, String)>,
    pub ports: Vec<PortSpec>,
    pub volumes: Vec<VolumeSpec>,
    pub config_hash: String,
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct BotError {
    message: String,
}

impl BotError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}
