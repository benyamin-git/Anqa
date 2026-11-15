pub mod status;

pub use status::{BotStatus, ContainerView, HealthStatus, from_inspect, uptime_seconds};

use bollard::API_DEFAULT_VERSION;
use bollard::Docker;
use bollard::errors::Error as BollardError;
use bollard::models::{ContainerInspectResponse, ContainerSummary};
use bollard::query_parameters::ListContainersOptionsBuilder;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct DockerClient {
    socket: PathBuf,
    inner: Arc<RwLock<Option<Docker>>>,
}

impl DockerClient {
    pub fn connect(socket: &Path) -> Self {
        let inner = match socket.to_str() {
            Some(path) => match Docker::connect_with_socket(path, 1800, API_DEFAULT_VERSION) {
                Ok(docker) => Some(docker),
                Err(error) => {
                    tracing::warn!(%error, "Docker socket is unavailable; requests will retry");
                    None
                }
            },
            None => {
                tracing::warn!("Docker socket path is not valid UTF-8");
                None
            }
        };

        Self {
            socket: socket.to_path_buf(),
            inner: Arc::new(RwLock::new(inner)),
        }
    }

    pub fn is_available(&self) -> bool {
        self.inner
            .try_read()
            .map(|guard| guard.is_some())
            .unwrap_or(false)
    }

    pub async fn try_negotiate(self) -> Self {
        let client = self.inner.read().await.clone();
        if let Some(client) = client {
            match client.negotiate_version().await {
                Ok(negotiated) => {
                    *self.inner.write().await = Some(negotiated);
                }
                Err(error) => {
                    tracing::warn!(%error, "Docker version negotiation failed; keeping defaults");
                }
            }
        }
        self
    }

    pub async fn ping(&self) -> Result<(), DockerError> {
        let client = self.client().await?;
        client.ping().await.map(|_| ()).map_err(map_bollard_error)
    }

    pub async fn list_managed(&self) -> Result<Vec<ContainerSummary>, DockerError> {
        let client = self.client().await?;
        let mut filters: HashMap<String, Vec<String>> = HashMap::new();
        filters.insert("label".to_string(), vec!["anqa.managed=true".to_string()]);
        let options = ListContainersOptionsBuilder::default()
            .all(true)
            .filters(&filters)
            .build();
        client
            .list_containers(Some(options))
            .await
            .map_err(map_bollard_error)
    }

    pub async fn inspect(
        &self,
        id_or_name: &str,
    ) -> Result<Option<ContainerInspectResponse>, DockerError> {
        let client = self.client().await?;
        match client.inspect_container(id_or_name, None).await {
            Ok(inspect) => Ok(Some(inspect)),
            Err(BollardError::DockerResponseServerError {
                status_code: 404, ..
            }) => Ok(None),
            Err(error) => Err(map_bollard_error(error)),
        }
    }

    async fn client(&self) -> Result<Docker, DockerError> {
        if let Some(client) = self.inner.read().await.clone() {
            return Ok(client);
        }

        let Some(path) = self.socket.to_str() else {
            return Err(DockerError::Unavailable);
        };
        let docker =
            Docker::connect_with_socket(path, 1800, API_DEFAULT_VERSION).map_err(|error| {
                tracing::debug!(%error, "Docker socket is still unavailable");
                DockerError::Unavailable
            })?;
        *self.inner.write().await = Some(docker.clone());
        tracing::info!("connected to Docker");
        Ok(docker)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DockerError {
    #[error("Docker is unavailable")]
    Unavailable,
    #[error("Docker conflict: {message}")]
    Conflict { message: String },
    #[error("Docker API error: {message}")]
    Api { message: String },
}

fn map_bollard_error(error: BollardError) -> DockerError {
    match error {
        BollardError::DockerResponseServerError {
            status_code: 409,
            message,
        } => DockerError::Conflict { message },
        BollardError::DockerResponseServerError { message, .. } => DockerError::Api { message },
        other => {
            tracing::debug!(error = %other, "Docker request failed");
            DockerError::Unavailable
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_socket_is_unavailable_but_not_fatal() {
        let client = DockerClient::connect(Path::new("/definitely/not/a/docker.sock"));
        assert!(!client.is_available());
    }

    #[test]
    fn error_debug_does_not_include_a_socket_path() {
        let debug = format!("{:?}", DockerError::Unavailable);
        assert!(!debug.contains("/var/run/docker.sock"));
        assert_eq!(
            format!(
                "{:?}",
                DockerError::Conflict {
                    message: "name already in use".to_string()
                }
            ),
            "Conflict { message: \"name already in use\" }"
        );
    }
}
