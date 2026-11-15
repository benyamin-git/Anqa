use ignore::gitignore::GitignoreBuilder;
use std::ffi::OsStr;
use std::fs::{self};
use std::io;
use std::path::{Component, Path, PathBuf};
use tar::{Builder, EntryType, Header, HeaderMode};
use walkdir::WalkDir;

#[derive(Debug, thiserror::Error)]
pub enum ContextError {
    #[error("build context root is not a directory: {0}")]
    NotDirectory(PathBuf),
    #[error("cannot build context ignore rules: {0}")]
    Dockerignore(String),
    #[error("cannot walk build context: {0}")]
    Walk(String),
    #[error("cannot archive {path}: {source}")]
    Archive { path: PathBuf, source: io::Error },
    #[error("cannot finish build context archive: {0}")]
    Finish(io::Error),
}

enum Kind {
    File,
    Directory,
    Symlink,
}

struct Found {
    absolute: PathBuf,
    relative: PathBuf,
    kind: Kind,
}

pub fn build_context(root: &Path) -> Result<Vec<u8>, ContextError> {
    if !root.is_dir() {
        return Err(ContextError::NotDirectory(root.to_path_buf()));
    }

    let mut ignore_builder = GitignoreBuilder::new(root);
    ignore_builder.allow_unclosed_class(false);
    let dockerignore = root.join(".dockerignore");
    if dockerignore.is_file()
        && let Some(error) = ignore_builder.add(&dockerignore)
    {
        return Err(ContextError::Dockerignore(error.to_string()));
    }
    let ignore = ignore_builder
        .build()
        .map_err(|error| ContextError::Dockerignore(error.to_string()))?;

    let mut found = Vec::new();
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry.map_err(|error| ContextError::Walk(error.to_string()))?;
        let relative = entry
            .path()
            .strip_prefix(root)
            .map_err(|error| ContextError::Walk(error.to_string()))?
            .to_path_buf();
        if relative.as_os_str().is_empty() {
            continue;
        }
        let file_type = entry.file_type();
        let kind = if file_type.is_dir() {
            Kind::Directory
        } else if file_type.is_file() {
            Kind::File
        } else if file_type.is_symlink() {
            Kind::Symlink
        } else {
            tracing::debug!(path = %relative.display(), "skipping non-regular build context entry");
            continue;
        };

        if hard_excluded(&relative) {
            continue;
        }
        let forced_dockerfile = relative == Path::new("Dockerfile");
        if !forced_dockerfile
            && ignore
                .matched_path_or_any_parents(&relative, matches!(kind, Kind::Directory))
                .is_ignore()
        {
            continue;
        }

        found.push(Found {
            absolute: entry.path().to_path_buf(),
            relative,
            kind,
        });
    }

    found.sort_by(|left, right| left.relative.cmp(&right.relative));

    let mut builder = Builder::new(Vec::new());
    builder.follow_symlinks(false);
    builder.mode(HeaderMode::Deterministic);
    for entry in found {
        match entry.kind {
            Kind::Directory => {
                builder
                    .append_dir(&entry.relative, &entry.absolute)
                    .map_err(|source| ContextError::Archive {
                        path: entry.relative,
                        source,
                    })?;
            }
            Kind::File => {
                builder
                    .append_path_with_name(&entry.absolute, &entry.relative)
                    .map_err(|source| ContextError::Archive {
                        path: entry.relative,
                        source,
                    })?;
            }
            Kind::Symlink => append_symlink(&mut builder, &entry)?,
        }
    }
    builder.finish().map_err(ContextError::Finish)?;
    builder.into_inner().map_err(ContextError::Finish)
}

fn append_symlink(builder: &mut Builder<Vec<u8>>, entry: &Found) -> Result<(), ContextError> {
    let target = fs::read_link(&entry.absolute).map_err(|source| ContextError::Archive {
        path: entry.relative.clone(),
        source,
    })?;
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Symlink);
    header.set_size(0);
    header.set_mode(0o777);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(tar::DETERMINISTIC_TIMESTAMP);
    builder
        .append_link(&mut header, &entry.relative, target)
        .map_err(|source| ContextError::Archive {
            path: entry.relative.clone(),
            source,
        })
}

fn hard_excluded(relative: &Path) -> bool {
    let name = relative.file_name();
    if name == Some(OsStr::new(".env")) || name == Some(OsStr::new(".dockerignore")) {
        return true;
    }
    relative.components().any(|component| match component {
        Component::Normal(name) => name == OsStr::new(".git"),
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Cursor;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use tar::{Archive, EntryType};
    use tempfile::TempDir;

    fn write(root: &Path, relative: &str, contents: &str) {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    fn entries(root: &Path) -> Vec<(String, EntryType, u32)> {
        let bytes = build_context(root).unwrap();
        let mut archive = Archive::new(Cursor::new(bytes));
        let mut entries = Vec::new();
        for entry in archive.entries().unwrap() {
            let entry = entry.unwrap();
            let path = entry.path().unwrap().to_string_lossy().into_owned();
            let entry_type = entry.header().entry_type();
            let mode = entry.header().mode().unwrap() & 0o777;
            entries.push((path, entry_type, mode));
        }
        entries
    }

    fn names(root: &Path) -> Vec<String> {
        entries(root).into_iter().map(|(path, _, _)| path).collect()
    }

    #[test]
    fn hard_excludes_and_forced_dockerfile_are_applied() {
        let temp = TempDir::new().unwrap();
        write(temp.path(), "Dockerfile", "FROM alpine:3.22\n");
        write(temp.path(), "app.py", "print('hi')\n");
        write(temp.path(), ".env", "SECRET=1\n");
        write(temp.path(), "sub/.env", "SECRET=2\n");
        write(temp.path(), ".env.example", "SECRET=example\n");
        write(temp.path(), ".git/config", "git\n");
        write(temp.path(), ".dockerignore", "Dockerfile\napp.py\n");
        write(temp.path(), "sub/keep.txt", "keep\n");

        let names = names(temp.path());
        assert!(names.contains(&"Dockerfile".to_string()));
        assert!(names.contains(&".env.example".to_string()));
        assert!(names.contains(&"sub/keep.txt".to_string()));
        assert!(!names.contains(&".env".to_string()));
        assert!(!names.contains(&"sub/.env".to_string()));
        assert!(!names.iter().any(|name| name.starts_with(".git/")));
        assert!(!names.contains(&".dockerignore".to_string()));
    }

    #[test]
    fn dockerignore_negation_reincludes_a_file() {
        let temp = TempDir::new().unwrap();
        write(temp.path(), "Dockerfile", "FROM alpine:3.22\n");
        write(temp.path(), ".dockerignore", "*.log\n!keep.log\n");
        write(temp.path(), "drop.log", "drop\n");
        write(temp.path(), "keep.log", "keep\n");
        let names = names(temp.path());
        assert!(names.contains(&"keep.log".to_string()));
        assert!(!names.contains(&"drop.log".to_string()));
    }

    #[test]
    fn nested_directories_are_preserved_and_paths_are_sorted() {
        let temp = TempDir::new().unwrap();
        write(temp.path(), "Dockerfile", "FROM alpine:3.22\n");
        write(temp.path(), "b.txt", "b\n");
        write(temp.path(), "a/deep/file.txt", "a\n");
        let names = names(temp.path());
        assert!(names.contains(&"a".to_string()));
        assert!(names.contains(&"a/deep".to_string()));
        assert!(names.contains(&"a/deep/file.txt".to_string()));
        assert_eq!(names, {
            let mut sorted = names.clone();
            sorted.sort();
            sorted
        });
    }

    #[test]
    fn empty_directories_are_archived() {
        let temp = TempDir::new().unwrap();
        fs::create_dir(temp.path().join("empty")).unwrap();
        let entries = entries(temp.path());
        assert!(
            entries
                .iter()
                .any(|(path, kind, _)| path == "empty" && kind.is_dir())
        );
    }

    #[test]
    fn executable_modes_are_preserved() {
        let temp = TempDir::new().unwrap();
        let script = temp.path().join("run.sh");
        fs::write(&script, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let entries = entries(temp.path());
        let (_, kind, mode) = entries
            .iter()
            .find(|(path, _, _)| path == "run.sh")
            .unwrap();
        assert!(kind.is_file());
        assert_eq!(*mode, 0o755);
    }

    #[test]
    fn symlinks_are_archived_as_links_not_followed() {
        let temp = TempDir::new().unwrap();
        write(temp.path(), "target.txt", "target\n");
        std::os::unix::fs::symlink("target.txt", temp.path().join("link.txt")).unwrap();
        let bytes = build_context(temp.path()).unwrap();
        let mut archive = Archive::new(Cursor::new(bytes));
        let mut found = false;
        for entry in archive.entries().unwrap() {
            let entry = entry.unwrap();
            if entry.path().unwrap() == Path::new("link.txt") {
                assert!(entry.header().entry_type().is_symlink());
                assert_eq!(
                    entry.link_name().unwrap().unwrap().as_os_str(),
                    std::ffi::OsStr::new("target.txt")
                );
                found = true;
            }
        }
        assert!(found);
    }

    #[test]
    fn invalid_dockerignore_patterns_are_errors() {
        let temp = TempDir::new().unwrap();
        write(temp.path(), ".dockerignore", "[\n");
        assert!(build_context(temp.path()).is_err());
    }
}
