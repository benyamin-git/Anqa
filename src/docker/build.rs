use bollard::body_full;
use bollard::errors::Error as BollardError;
use bollard::query_parameters::{
    BuildImageOptionsBuilder, BuilderVersion, CreateImageOptionsBuilder,
};
use futures_util::StreamExt;
use std::collections::HashMap;
use std::time::Duration;

use super::context::{ContextError, build_context};
use super::{DockerClient, DockerError, map_bollard_error};
use crate::bots::{BotSource, ResolvedBot};

const BUILD_TIMEOUT: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("Docker error: {0}")]
    Docker(#[from] DockerError),
    #[error("build context error: {0}")]
    Context(#[from] ContextError),
    #[error("build task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("invalid image reference: {0}")]
    InvalidImage(String),
    #[error("image build failed: {0}")]
    Build(String),
    #[error("image pull failed: {0}")]
    Pull(String),
    #[error("image {0} was not found after the operation")]
    ImageMissing(String),
    #[error("Docker image operation timed out")]
    Timeout,
}

pub async fn ensure_image(docker: &DockerClient, bot: &ResolvedBot) -> Result<String, BuildError> {
    match bot.source {
        BotSource::Dockerfile => build_dockerfile(docker, bot).await,
        BotSource::Image => pull_image(docker, &bot.image).await,
    }
}

pub async fn build_dockerfile(
    docker: &DockerClient,
    bot: &ResolvedBot,
) -> Result<String, BuildError> {
    let context_root = bot.root.join(&bot.dir);
    let tar = tokio::task::spawn_blocking(move || build_context(&context_root)).await??;

    let session = random_session_id(&bot.slug)?;
    let client = docker.client().await?;
    let labels = HashMap::from([
        ("anqa.managed".to_string(), "true".to_string()),
        (
            "anqa.version".to_string(),
            env!("CARGO_PKG_VERSION").to_string(),
        ),
    ]);
    let options = BuildImageOptionsBuilder::default()
        .dockerfile("Dockerfile")
        .t(&bot.image)
        .version(BuilderVersion::BuilderBuildKit)
        .session(&session)
        .rm(true)
        .labels(&labels)
        .build();

    let stream = client.build_image(options, None, Some(body_full(tar.into())));
    let consumed = async {
        let mut stream = stream;
        while let Some(item) = stream.next().await {
            let info = item.map_err(|error| BuildError::Build(error.to_string()))?;
            log_build_info(&info);
        }
        Ok::<(), BuildError>(())
    };
    tokio::time::timeout(BUILD_TIMEOUT, consumed)
        .await
        .map_err(|_| BuildError::Timeout)??;

    let image_id = inspect_image_id(docker, &bot.image).await?;
    image_id.ok_or_else(|| BuildError::ImageMissing(bot.image.clone()))
}

pub async fn pull_image(docker: &DockerClient, image: &str) -> Result<String, BuildError> {
    if let Some(image_id) = inspect_image_id(docker, image).await? {
        return Ok(image_id);
    }

    let (repository, tag) = split_image_ref(image)?;
    let client = docker.client().await?;
    let options = CreateImageOptionsBuilder::default()
        .from_image(&repository)
        .tag(&tag)
        .build();
    let stream = client.create_image(Some(options), None, None);
    let consumed = async {
        let mut stream = stream;
        while let Some(item) = stream.next().await {
            item.map_err(|error| BuildError::Pull(error.to_string()))?;
        }
        Ok::<(), BuildError>(())
    };
    tokio::time::timeout(BUILD_TIMEOUT, consumed)
        .await
        .map_err(|_| BuildError::Timeout)??;

    let image_id = inspect_image_id(docker, image).await?;
    image_id.ok_or_else(|| BuildError::ImageMissing(image.to_string()))
}

async fn inspect_image_id(
    docker: &DockerClient,
    image: &str,
) -> Result<Option<String>, BuildError> {
    let client = docker.client().await?;
    match client.inspect_image(image).await {
        Ok(inspect) => Ok(inspect.id),
        Err(BollardError::DockerResponseServerError {
            status_code: 404, ..
        }) => Ok(None),
        Err(error) => Err(BuildError::Docker(map_bollard_error(error))),
    }
}

fn random_session_id(slug: &str) -> Result<String, BuildError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|error| BuildError::Build(format!("cannot create session ID: {error}")))?;
    Ok(format!("anqa-{slug}-{}", hex::encode(bytes)))
}

fn log_build_info(info: &bollard::models::BuildInfo) {
    if let Some(status) = &info.status {
        tracing::debug!(status = %status, "image build status");
    }
    if let Some(stream) = &info.stream {
        tracing::debug!(stream = %truncate_log(stream), "image build output");
    }
}

fn truncate_log(value: &str) -> &str {
    if value.len() <= 2048 {
        return value;
    }
    let mut end = 2048;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

pub fn split_image_ref(image: &str) -> Result<(String, String), BuildError> {
    let image = image.trim();
    if image.is_empty() {
        return Err(BuildError::InvalidImage(image.to_string()));
    }

    if let Some((repository, digest)) = image.split_once('@') {
        if repository.is_empty() || digest.is_empty() {
            return Err(BuildError::InvalidImage(image.to_string()));
        }
        return Ok((repository.to_string(), digest.to_string()));
    }

    let last_slash = image.rfind('/');
    let last_colon = image.rfind(':');
    if let Some(colon) = last_colon.filter(|colon| last_slash.is_none_or(|slash| *colon > slash)) {
        let repository = &image[..colon];
        let tag = &image[colon + 1..];
        if repository.is_empty() || tag.is_empty() {
            return Err(BuildError::InvalidImage(image.to_string()));
        }
        return Ok((repository.to_string(), tag.to_string()));
    }

    Ok((image.to_string(), "latest".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_image_ref_handles_names_tags_ports_and_digests() {
        assert_eq!(
            split_image_ref("alpine").unwrap(),
            ("alpine".to_string(), "latest".to_string())
        );
        assert_eq!(
            split_image_ref("alpine:3.22").unwrap(),
            ("alpine".to_string(), "3.22".to_string())
        );
        assert_eq!(
            split_image_ref("ghcr.io/benyamin/echo:0.1.0").unwrap(),
            ("ghcr.io/benyamin/echo".to_string(), "0.1.0".to_string())
        );
        assert_eq!(
            split_image_ref("localhost:5000/team/echo").unwrap(),
            ("localhost:5000/team/echo".to_string(), "latest".to_string())
        );
        assert_eq!(
            split_image_ref("localhost:5000/team/echo:dev").unwrap(),
            ("localhost:5000/team/echo".to_string(), "dev".to_string())
        );
        assert_eq!(
            split_image_ref("ghcr.io/team/echo@sha256:abc123").unwrap(),
            ("ghcr.io/team/echo".to_string(), "sha256:abc123".to_string())
        );
    }

    #[test]
    fn empty_image_references_are_rejected() {
        assert!(split_image_ref("").is_err());
        assert!(split_image_ref(":tag").is_err());
        assert!(split_image_ref("repo:").is_err());
        assert!(split_image_ref("@sha256:abc").is_err());
    }
}
