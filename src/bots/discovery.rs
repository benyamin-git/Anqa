use super::slug::slugify;
use super::{ResolvedBot, load_bot};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotErrorCard {
    pub dir: String,
    pub message: String,
}

pub struct Discovery {
    pub bots: Vec<ResolvedBot>,
    pub errors: Vec<BotErrorCard>,
}

#[derive(Debug, thiserror::Error)]
#[error("cannot read bots directory {path}: {source}")]
pub struct ScanError {
    path: PathBuf,
    source: io::Error,
}

struct Candidate {
    dir: String,
    slug: Result<String, super::slug::SlugError>,
}

pub fn scan(root: &Path) -> Result<Discovery, ScanError> {
    let entries = fs::read_dir(root).map_err(|source| ScanError {
        path: root.to_path_buf(),
        source,
    })?;

    let mut candidates = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| ScanError {
            path: root.to_path_buf(),
            source,
        })?;
        let file_type = entry.file_type().map_err(|source| ScanError {
            path: root.to_path_buf(),
            source,
        })?;
        if !file_type.is_dir() {
            continue;
        }
        let Some(dir) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if dir.starts_with('.') {
            continue;
        }
        let path = entry.path();
        if !path.join("Dockerfile").is_file() && !path.join("bot.toml").is_file() {
            continue;
        }
        candidates.push(Candidate {
            slug: slugify(&dir),
            dir,
        });
    }

    let mut slug_counts: HashMap<String, usize> = HashMap::new();
    for candidate in &candidates {
        if let Ok(slug) = &candidate.slug {
            *slug_counts.entry(slug.clone()).or_default() += 1;
        }
    }

    let mut bots = Vec::new();
    let mut errors = Vec::new();
    for candidate in candidates {
        let dir = candidate.dir;
        match candidate.slug {
            Err(error) => errors.push(BotErrorCard {
                dir,
                message: format!("invalid bot directory: {error}"),
            }),
            Ok(slug) if slug_counts.get(&slug).copied().unwrap_or(0) > 1 => {
                errors.push(BotErrorCard {
                    dir,
                    message: "directory names map to the same Docker slug; rename one".to_string(),
                });
            }
            Ok(_) => match load_bot(root, &dir) {
                Ok(bot) => bots.push(bot),
                Err(error) => errors.push(BotErrorCard {
                    dir,
                    message: error.to_string(),
                }),
            },
        }
    }

    bots.sort_by(|left, right| left.dir.cmp(&right.dir));
    errors.sort_by(|left, right| left.dir.cmp(&right.dir));
    Ok(Discovery { bots, errors })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bots::BotSource;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    fn bot(root: &Path, dir: &str, manifest: Option<&str>, dockerfile: bool) {
        let path = root.join(dir);
        fs::create_dir(&path).unwrap();
        if let Some(manifest) = manifest {
            fs::write(path.join("bot.toml"), manifest).unwrap();
        }
        if dockerfile {
            fs::write(path.join("Dockerfile"), "FROM alpine:3.22\n").unwrap();
        }
    }

    #[test]
    fn empty_and_non_bot_directories_are_ignored() {
        let temp = TempDir::new().unwrap();
        fs::create_dir(temp.path().join("empty")).unwrap();
        fs::write(temp.path().join("plain-file"), "not a bot").unwrap();
        let discovery = scan(temp.path()).unwrap();
        assert!(discovery.bots.is_empty());
        assert!(discovery.errors.is_empty());
    }

    #[test]
    fn dockerfile_only_and_manifest_only_bots_are_found() {
        let temp = TempDir::new().unwrap();
        bot(temp.path(), "docker-bot", None, true);
        bot(
            temp.path(),
            "image-bot",
            Some("image = \"ghcr.io/you/echo:0.1.0\"\n"),
            false,
        );
        let discovery = scan(temp.path()).unwrap();
        assert_eq!(discovery.bots.len(), 2);
        assert_eq!(discovery.bots[0].dir, "docker-bot");
        assert_eq!(discovery.bots[0].source, BotSource::Dockerfile);
        assert_eq!(discovery.bots[1].dir, "image-bot");
        assert_eq!(discovery.bots[1].source, BotSource::Image);
        assert!(discovery.errors.is_empty());
    }

    #[test]
    fn hidden_symlink_and_nested_entries_are_ignored() {
        let temp = TempDir::new().unwrap();
        bot(temp.path(), ".hidden", None, true);
        bot(temp.path(), "real", None, true);
        fs::create_dir(temp.path().join("parent")).unwrap();
        bot(temp.path().join("parent").as_path(), "child", None, true);
        #[cfg(unix)]
        std::os::unix::fs::symlink(temp.path().join("real"), temp.path().join("link")).unwrap();

        let discovery = scan(temp.path()).unwrap();
        assert_eq!(discovery.bots.len(), 1);
        assert_eq!(discovery.bots[0].dir, "real");
        assert!(discovery.errors.is_empty());
    }

    #[test]
    fn an_invalid_manifest_is_a_card_without_aborting_the_scan() {
        let temp = TempDir::new().unwrap();
        bot(temp.path(), "broken", Some("wat = true\n"), true);
        bot(temp.path(), "good", None, true);
        let discovery = scan(temp.path()).unwrap();
        assert_eq!(discovery.bots.len(), 1);
        assert_eq!(discovery.bots[0].dir, "good");
        assert_eq!(discovery.errors.len(), 1);
        assert_eq!(discovery.errors[0].dir, "broken");
        assert!(!discovery.errors[0].message.is_empty());
    }

    #[test]
    fn colliding_slugs_error_all_colliding_directories() {
        let temp = TempDir::new().unwrap();
        bot(temp.path(), "Echo One", None, true);
        bot(temp.path(), "echo-one", None, true);
        bot(temp.path(), "other", None, true);
        let discovery = scan(temp.path()).unwrap();
        assert_eq!(discovery.bots.len(), 1);
        assert_eq!(discovery.bots[0].dir, "other");
        assert_eq!(discovery.errors.len(), 2);
        assert_eq!(discovery.errors[0].dir, "Echo One");
        assert_eq!(discovery.errors[1].dir, "echo-one");
        assert!(
            discovery.errors[0]
                .message
                .contains("directory names map to the same Docker slug")
        );
    }

    #[test]
    fn results_are_sorted_by_directory_name() {
        let temp = TempDir::new().unwrap();
        bot(temp.path(), "z-bot", None, true);
        bot(temp.path(), "a-bot", None, true);
        bot(temp.path(), "broken", Some("name = 3\n"), true);
        let discovery = scan(temp.path()).unwrap();
        assert_eq!(
            discovery
                .bots
                .iter()
                .map(|bot| bot.dir.as_str())
                .collect::<Vec<_>>(),
            ["a-bot", "z-bot"]
        );
        assert_eq!(
            discovery
                .errors
                .iter()
                .map(|error| error.dir.as_str())
                .collect::<Vec<_>>(),
            ["broken"]
        );
    }

    #[test]
    fn missing_root_is_an_error() {
        let temp = TempDir::new().unwrap();
        assert!(scan(&temp.path().join("missing")).is_err());
    }
}
