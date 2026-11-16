use bollard::query_parameters::RemoveContainerOptionsBuilder;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path};

use super::{DockerClient, DockerError, map_bollard_error};

const SENTINEL_FILE: &str = ".anqa-volume";
const BOT_LABEL: &str = "anqa.bot";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct CleanupReport {
    pub skipped: bool,
    pub removed: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum CleanupError {
    #[error("cannot prepare the orphan-cleanup sentinel: {0}")]
    Sentinel(#[from] std::io::Error),
    #[error("Docker error: {0}")]
    Docker(#[from] DockerError),
}

pub async fn ensure_orphan_cleanup(
    docker: &DockerClient,
    bots_dir: &Path,
    data_dir: &Path,
) -> Result<CleanupReport, CleanupError> {
    if !prepare_cleanup(bots_dir, data_dir)? {
        return Ok(CleanupReport {
            skipped: true,
            ..CleanupReport::default()
        });
    }

    let containers = docker.list_managed().await?;
    let client = docker.client().await?;
    let mut report = CleanupReport::default();

    for container in containers {
        let label = container
            .labels
            .as_ref()
            .and_then(|labels| labels.get(BOT_LABEL))
            .map(String::as_str);
        if !label.is_some_and(bot_label_is_valid) {
            tracing::warn!(
                container = container_name(&container),
                label = label.unwrap_or("<missing>"),
                "removing managed container with an invalid bot label"
            );
        } else if !should_remove_orphan(bots_dir, label.expect("validated label")) {
            continue;
        }

        let name = container_name(&container);
        let Some(id) = container.id.as_deref().filter(|id| !id.is_empty()) else {
            let error = format!("{name}: container has no ID");
            tracing::warn!(%error, "orphan cleanup failed");
            report.errors.push(error);
            continue;
        };

        let options = RemoveContainerOptionsBuilder::default()
            .force(true)
            .v(false)
            .build();
        match client.remove_container(id, Some(options)).await {
            Ok(()) => {
                tracing::info!(container = %name, "removed orphaned bot container");
                report.removed.push(name);
            }
            Err(error) => {
                let error = format!("{name}: {}", map_bollard_error(error));
                tracing::warn!(%error, "orphan cleanup failed");
                report.errors.push(error);
            }
        }
    }

    Ok(report)
}

fn prepare_cleanup(bots_dir: &Path, data_dir: &Path) -> Result<bool, std::io::Error> {
    let Ok(metadata) = fs::metadata(bots_dir) else {
        tracing::warn!(path = %bots_dir.display(), "bots root is unreadable; skipping orphan cleanup");
        return Ok(false);
    };
    if !metadata.is_dir() {
        tracing::warn!(path = %bots_dir.display(), "bots root is not a directory; skipping orphan cleanup");
        return Ok(false);
    }

    let fingerprint = format!("{}:{}", metadata.dev(), metadata.ino());
    let sentinel = data_dir.join(SENTINEL_FILE);
    let current = match fs::read_to_string(&sentinel) {
        Ok(current) => Some(current),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };

    if current.as_deref().map(str::trim) == Some(fingerprint.as_str()) {
        return Ok(true);
    }

    fs::write(&sentinel, fingerprint)?;
    Ok(false)
}

fn bot_label_is_valid(label: &str) -> bool {
    if label.is_empty()
        || label == ".."
        || label.contains('/')
        || label.contains('\\')
        || label.chars().any(char::is_control)
    {
        return false;
    }
    let mut components = Path::new(label).components();
    matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(_)), None)
    )
}

fn should_remove_orphan(bots_dir: &Path, label: &str) -> bool {
    if !bot_label_is_valid(label) {
        return true;
    }

    match fs::symlink_metadata(bots_dir.join(label)) {
        Ok(metadata) => !metadata.file_type().is_dir() || metadata.file_type().is_symlink(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(error) => {
            tracing::warn!(%error, label, "cannot inspect a bot directory; keeping the container");
            false
        }
    }
}

fn container_name(container: &bollard::models::ContainerSummary) -> String {
    container
        .names
        .as_ref()
        .and_then(|names| names.first())
        .map(|name| name.trim_start_matches('/'))
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .or_else(|| container.id.clone())
        .unwrap_or_else(|| "<unknown>".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn roots() -> (TempDir, PathBuf, PathBuf) {
        let temp = TempDir::new().unwrap();
        let bots = temp.path().join("bots");
        let data = temp.path().join("data");
        fs::create_dir(&bots).unwrap();
        fs::create_dir(&data).unwrap();
        (temp, bots, data)
    }

    fn fingerprint(bots: &Path) -> String {
        let metadata = fs::metadata(bots).unwrap();
        format!("{}:{}", metadata.dev(), metadata.ino())
    }

    #[test]
    fn first_sentinel_write_skips_cleanup() {
        let (_temp, bots, data) = roots();
        assert!(!prepare_cleanup(&bots, &data).unwrap());
        assert_eq!(
            fs::read_to_string(data.join(SENTINEL_FILE)).unwrap(),
            fingerprint(&bots)
        );
    }

    #[test]
    fn a_changed_fingerprint_rewrites_the_sentinel_and_skips() {
        let (_temp, bots, data) = roots();
        fs::write(data.join(SENTINEL_FILE), "1:2").unwrap();
        assert!(!prepare_cleanup(&bots, &data).unwrap());
        assert_eq!(
            fs::read_to_string(data.join(SENTINEL_FILE)).unwrap(),
            fingerprint(&bots)
        );
    }

    #[test]
    fn the_same_fingerprint_runs_cleanup() {
        let (_temp, bots, data) = roots();
        assert!(!prepare_cleanup(&bots, &data).unwrap());
        assert!(prepare_cleanup(&bots, &data).unwrap());
    }

    #[test]
    fn an_unreadable_root_skips_without_writing() {
        let temp = TempDir::new().unwrap();
        let data = temp.path().join("data");
        fs::create_dir(&data).unwrap();
        let missing = temp.path().join("missing");
        assert!(!prepare_cleanup(&missing, &data).unwrap());
        assert!(!data.join(SENTINEL_FILE).exists());
    }

    #[test]
    fn invalid_bot_labels_are_rejected_before_joining() {
        for label in ["", ".", "..", "/", "\\", "a/b", "a\\b", "a/../b", "a\nb"] {
            assert!(!bot_label_is_valid(label), "{label:?}");
            assert!(should_remove_orphan(Path::new("/tmp"), label), "{label:?}");
        }
        assert!(bot_label_is_valid("echo-bot"));
        assert!(bot_label_is_valid("Echo Bot"));
    }

    #[test]
    fn only_real_directories_are_kept() {
        let (_temp, bots, _data) = roots();
        fs::create_dir(bots.join("live")).unwrap();
        fs::write(bots.join("file"), "x").unwrap();
        std::os::unix::fs::symlink(bots.join("live"), bots.join("link")).unwrap();

        assert!(!should_remove_orphan(&bots, "live"));
        assert!(should_remove_orphan(&bots, "missing"));
        assert!(should_remove_orphan(&bots, "file"));
        assert!(should_remove_orphan(&bots, "link"));
    }

    #[test]
    fn cleanup_report_starts_empty_and_unskipped() {
        let report = CleanupReport::default();
        assert!(!report.skipped);
        assert!(report.removed.is_empty());
        assert!(report.errors.is_empty());
    }
}
