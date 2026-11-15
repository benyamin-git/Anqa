use super::slug::slugify;
use super::{BotError, BotSource, Manifest, PortProtocol, PortSpec, ResolvedBot, VolumeSpec};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::net::{IpAddr, Ipv6Addr};
use std::path::{Component, Path, PathBuf};

const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

pub fn load_bot(root: &Path, dir: &str) -> Result<ResolvedBot, BotError> {
    validate_dir(dir)?;
    let bot_root = root.join(dir);
    if !bot_root.is_dir() {
        return Err(BotError::new(format!(
            "bot directory {} does not exist",
            bot_root.display()
        )));
    }

    let manifest_path = bot_root.join("bot.toml");
    let manifest = if manifest_path.is_file() {
        let contents = read_limited(&manifest_path)?;
        toml::from_str(&contents).map_err(|error| {
            BotError::new(format!(
                "invalid manifest {}: {error}",
                manifest_path.display()
            ))
        })?
    } else if manifest_path.exists() {
        return Err(BotError::new(format!(
            "manifest {} is not a file",
            manifest_path.display()
        )));
    } else {
        Manifest::default()
    };

    if manifest
        .command
        .as_ref()
        .is_some_and(|command| command.is_empty())
    {
        return Err(BotError::new(
            "manifest command must be a non-empty argv array",
        ));
    }

    let has_dockerfile = bot_root.join("Dockerfile").is_file();
    let image_override = match manifest.image.as_deref() {
        Some(value) => {
            let image = value.trim();
            if image.is_empty() {
                return Err(BotError::new("manifest image must not be empty"));
            }
            validate_image(image)?;
            Some(image.to_string())
        }
        None => None,
    };

    if image_override.is_some() && has_dockerfile {
        return Err(BotError::new(
            "manifest image and Dockerfile are mutually exclusive",
        ));
    }
    if image_override.is_none() && !has_dockerfile {
        return Err(BotError::new(
            "bot directory needs a Dockerfile or a manifest image",
        ));
    }

    let slug = slugify(dir)
        .map_err(|error| BotError::new(format!("invalid bot directory {dir:?}: {error}")))?;
    let name = manifest
        .name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(dir)
        .to_string();
    let env = load_env(&bot_root, &manifest)?;
    let ports = parse_ports(&manifest.ports)?;
    let volumes = parse_volumes(&manifest.volumes)?;
    let (source, image) = match image_override {
        Some(image) => (BotSource::Image, image),
        None => (BotSource::Dockerfile, format!("anqa/{slug}:latest")),
    };
    let config_hash = config_hash(&image, &manifest, &env)?;

    Ok(ResolvedBot {
        dir: dir.to_string(),
        slug,
        name,
        image,
        source,
        manifest,
        env,
        ports,
        volumes,
        config_hash,
        root: root.to_path_buf(),
    })
}

fn validate_dir(dir: &str) -> Result<(), BotError> {
    if dir.is_empty()
        || dir == "."
        || dir == ".."
        || dir.contains('/')
        || dir.contains('\\')
        || Path::new(dir)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(BotError::new(format!("invalid bot directory {dir:?}")));
    }
    Ok(())
}

fn validate_image(image: &str) -> Result<(), BotError> {
    if image.len() > 255
        || image
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(BotError::new("manifest image is not a valid Docker image"));
    }
    Ok(())
}

fn read_limited(path: &Path) -> Result<String, BotError> {
    let file = File::open(path)
        .map_err(|error| BotError::new(format!("cannot read {}: {error}", path.display())))?;
    let mut bytes = Vec::new();
    file.take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| BotError::new(format!("cannot read {}: {error}", path.display())))?;
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        return Err(BotError::new(format!(
            "{} is larger than 1 MiB",
            path.display()
        )));
    }
    String::from_utf8(bytes)
        .map_err(|error| BotError::new(format!("{} is not UTF-8: {error}", path.display())))
}

fn load_env(bot_root: &Path, manifest: &Manifest) -> Result<Vec<(String, String)>, BotError> {
    let Some(path) = env_file_path(bot_root, manifest)? else {
        return Ok(Vec::new());
    };
    let contents = read_limited(&path)?;
    let mut environment = BTreeMap::new();
    for item in dotenvy::from_read_iter(contents.as_bytes()) {
        let (key, value) = item.map_err(|error| {
            BotError::new(format!("invalid env file {}: {error}", path.display()))
        })?;
        environment.insert(key, value);
    }
    Ok(environment.into_iter().collect())
}

fn env_file_path(bot_root: &Path, manifest: &Manifest) -> Result<Option<PathBuf>, BotError> {
    let (name, explicit) = match manifest.env_file.as_deref() {
        Some(name) => (name, true),
        None => (".env", false),
    };
    if name.is_empty() {
        if explicit {
            return Err(BotError::new("manifest env_file must not be empty"));
        }
        return Ok(None);
    }

    let relative = Path::new(name);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(BotError::new(format!(
            "env_file {name:?} must be a relative path without '..'"
        )));
    }

    let path = bot_root.join(relative);
    if path.is_file() {
        Ok(Some(path))
    } else if explicit {
        Err(BotError::new(format!(
            "env_file {} does not exist",
            path.display()
        )))
    } else if path.exists() {
        Err(BotError::new(format!(
            "env_file {} is not a file",
            path.display()
        )))
    } else {
        Ok(None)
    }
}

fn parse_ports(entries: &[String]) -> Result<Vec<PortSpec>, BotError> {
    let mut ports = Vec::with_capacity(entries.len());
    let mut host_ports = HashSet::new();
    let mut targets = HashSet::new();

    for raw in entries {
        let entry = raw.trim();
        if entry.is_empty() {
            return Err(BotError::new("port entries must not be empty"));
        }
        let (core, protocol) = match entry.rsplit_once('/') {
            Some((core, protocol)) => {
                let protocol = match protocol.to_ascii_lowercase().as_str() {
                    "tcp" => PortProtocol::Tcp,
                    "udp" => PortProtocol::Udp,
                    _ => {
                        return Err(BotError::new(format!("invalid port protocol in {raw:?}")));
                    }
                };
                (core, protocol)
            }
            None => (entry, PortProtocol::Tcp),
        };

        let parts: Vec<&str> = core.rsplitn(3, ':').collect();
        let (host_port, container_port, host_ip) = match parts.as_slice() {
            [container, host] => (host, container, None),
            [container, host, ip] => (host, container, Some(*ip)),
            _ => {
                return Err(BotError::new(format!(
                    "invalid port mapping {raw:?}; expected host:container"
                )));
            }
        };
        let host_port = parse_port(host_port, raw)?;
        let container_port = parse_port(container_port, raw)?;
        let host_ip = host_ip.map(normalize_host_ip).transpose()?;

        if !host_ports.insert(host_port) {
            return Err(BotError::new(format!(
                "duplicate host port {host_port} in {raw:?}"
            )));
        }
        if !targets.insert((container_port, protocol)) {
            return Err(BotError::new(format!(
                "duplicate container port {container_port} in {raw:?}"
            )));
        }

        ports.push(PortSpec {
            host_ip,
            host_port,
            container_port,
            protocol,
        });
    }

    Ok(ports)
}

fn parse_port(value: &str, raw: &str) -> Result<u16, BotError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(BotError::new(format!("invalid port mapping {raw:?}")));
    }
    let port = value
        .parse::<u16>()
        .map_err(|_| BotError::new(format!("invalid port mapping {raw:?}")))?;
    if port == 0 {
        return Err(BotError::new(format!("port 0 is not allowed in {raw:?}")));
    }
    Ok(port)
}

fn normalize_host_ip(raw: &str) -> Result<String, BotError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(BotError::new("host IP must not be empty"));
    }

    if raw.starts_with('[') || raw.ends_with(']') {
        if !(raw.starts_with('[') && raw.ends_with(']')) {
            return Err(BotError::new(format!("invalid bracketed host IP {raw:?}")));
        }
        let inner = &raw[1..raw.len() - 1];
        let address = inner
            .parse::<Ipv6Addr>()
            .map_err(|_| BotError::new(format!("invalid IPv6 host IP {raw:?}")))?;
        return Ok(address.to_string());
    }

    if raw.contains(':') {
        return Err(BotError::new(format!(
            "IPv6 host IPs must be bracketed: {raw:?}"
        )));
    }
    if let Ok(address) = raw.parse::<IpAddr>() {
        return Ok(address.to_string());
    }
    if is_hostname(raw) {
        return Ok(raw.to_string());
    }
    Err(BotError::new(format!("invalid host IP {raw:?}")))
}

fn is_hostname(value: &str) -> bool {
    value.len() <= 253
        && value
            .chars()
            .any(|character| character.is_ascii_alphabetic())
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_')
        })
}

fn parse_volumes(entries: &[String]) -> Result<Vec<VolumeSpec>, BotError> {
    let mut volumes = Vec::with_capacity(entries.len());
    let mut names = HashSet::new();
    let mut targets = HashSet::new();

    for raw in entries {
        let entry = raw.trim();
        let Some((name, target)) = entry.split_once(':') else {
            return Err(BotError::new(format!(
                "invalid volume {raw:?}; expected name:absolute/path"
            )));
        };
        if !is_volume_name(name) {
            return Err(BotError::new(format!(
                "invalid volume name {name:?} in {raw:?}"
            )));
        }
        if target.is_empty() || target.contains(':') {
            return Err(BotError::new(format!("invalid volume target in {raw:?}")));
        }
        let target_path = Path::new(target);
        if !target_path.is_absolute()
            || target_path
                .components()
                .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(BotError::new(format!(
                "volume target {target:?} must be an absolute path without '..'"
            )));
        }
        if !names.insert(name.to_string()) {
            return Err(BotError::new(format!("duplicate volume name {name:?}")));
        }
        if !targets.insert(target.to_string()) {
            return Err(BotError::new(format!("duplicate volume target {target:?}")));
        }

        volumes.push(VolumeSpec {
            name: name.to_string(),
            target: target_path.to_path_buf(),
        });
    }

    Ok(volumes)
}

fn is_volume_name(name: &str) -> bool {
    let mut characters = name.chars();
    matches!(characters.next(), Some(first) if first.is_ascii_alphanumeric())
        && name.len() <= 128
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
}

#[derive(Serialize)]
struct ConfigHashInput<'a> {
    image: &'a str,
    command: &'a Option<Vec<String>>,
    env: &'a [(String, String)],
    ports: &'a [String],
    volumes: &'a [String],
}

fn config_hash(
    image: &str,
    manifest: &Manifest,
    env: &[(String, String)],
) -> Result<String, BotError> {
    let input = ConfigHashInput {
        image,
        command: &manifest.command,
        env,
        ports: &manifest.ports,
        volumes: &manifest.volumes,
    };
    let bytes = serde_json::to_vec(&input)
        .map_err(|error| BotError::new(format!("cannot serialize bot config: {error}")))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn temp_bot(manifest: Option<&str>, dockerfile: bool) -> (TempDir, std::path::PathBuf) {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("echo-bot");
        fs::create_dir(&root).unwrap();
        if let Some(manifest) = manifest {
            fs::write(root.join("bot.toml"), manifest).unwrap();
        }
        if dockerfile {
            fs::write(root.join("Dockerfile"), "FROM alpine:3.22\n").unwrap();
        }
        (temp, root)
    }

    fn load(manifest: Option<&str>, dockerfile: bool) -> Result<ResolvedBot, BotError> {
        let (temp, _root) = temp_bot(manifest, dockerfile);
        load_bot(temp.path(), "echo-bot")
    }

    #[test]
    fn a_minimal_dockerfile_bot_gets_defaults() {
        let bot = load(None, true).unwrap();
        assert_eq!(bot.source, BotSource::Dockerfile);
        assert_eq!(bot.image, "anqa/echo-bot:latest");
        assert_eq!(bot.name, "echo-bot");
        assert!(bot.env.is_empty());
        assert!(bot.ports.is_empty());
        assert!(bot.volumes.is_empty());
        assert_eq!(bot.config_hash.len(), 64);
    }

    #[test]
    fn an_image_manifest_needs_no_dockerfile() {
        let bot = load(
            Some("name = \"Echo Bot\"\nimage = \"ghcr.io/you/echo:0.1.0\"\n"),
            false,
        )
        .unwrap();
        assert_eq!(bot.source, BotSource::Image);
        assert_eq!(bot.image, "ghcr.io/you/echo:0.1.0");
        assert_eq!(bot.name, "Echo Bot");
    }

    #[test]
    fn both_sources_and_neither_source_are_errors() {
        assert!(load(Some("image = \"alpine:3.22\"\n"), true).is_err());
        assert!(load(None, false).is_err());
        assert!(load(Some("name = \"Empty\"\n"), false).is_err());
    }

    #[test]
    fn unknown_fields_are_rejected() {
        assert!(load(Some("wat = true\n"), true).is_err());
    }

    #[test]
    fn command_must_be_a_nonempty_argv_array() {
        assert!(load(Some("command = []\n"), true).is_err());
        assert!(load(Some("command = \"echo hi\"\n"), true).is_err());
        let bot = load(Some("command = [\"sh\", \"-c\", \"echo hi\"]\n"), true).unwrap();
        assert_eq!(
            bot.manifest.command.as_deref(),
            Some(&["sh".to_string(), "-c".to_string(), "echo hi".to_string()][..])
        );
    }

    #[test]
    fn explicit_env_file_must_exist_but_default_env_is_optional() {
        assert!(load(Some("env_file = \"missing.env\"\n"), true).is_err());

        let (temp, root) = temp_bot(Some("env_file = \"config.env\"\n"), true);
        fs::write(root.join("config.env"), "TOKEN=abc\n").unwrap();
        let bot = load_bot(temp.path(), "echo-bot").unwrap();
        assert_eq!(bot.env, vec![("TOKEN".to_string(), "abc".to_string())]);
    }

    #[test]
    fn env_is_sorted_and_last_duplicate_wins() {
        let (temp, root) = temp_bot(None, true);
        fs::write(root.join(".env"), "B=2\nA=1\nA=3\n").unwrap();
        let bot = load_bot(temp.path(), "echo-bot").unwrap();
        assert_eq!(
            bot.env,
            vec![
                ("A".to_string(), "3".to_string()),
                ("B".to_string(), "2".to_string())
            ]
        );
    }

    #[test]
    fn invalid_port_forms_are_rejected() {
        for port in [
            "8080",
            "0:8080",
            "8080:0",
            "abc:8080",
            "8080:abc",
            "8080:8080/sctp",
            "::1:8080:8080",
            "8080:8080:8080",
        ] {
            let manifest = format!("ports = [\"{port}\"]\n");
            assert!(load(Some(&manifest), true).is_err(), "port {port:?}");
        }
    }

    #[test]
    fn valid_ports_and_duplicate_detection() {
        let bot = load(
            Some("ports = [\"127.0.0.1:8080:80\", \"[::1]:8443:443/udp\"]\n"),
            true,
        )
        .unwrap();
        assert_eq!(bot.ports.len(), 2);
        assert_eq!(bot.ports[0].host_ip.as_deref(), Some("127.0.0.1"));
        assert_eq!(bot.ports[0].host_port, 8080);
        assert_eq!(bot.ports[0].container_port, 80);
        assert_eq!(bot.ports[0].protocol, PortProtocol::Tcp);
        assert_eq!(bot.ports[1].host_ip.as_deref(), Some("::1"));
        assert_eq!(bot.ports[1].protocol, PortProtocol::Udp);

        assert!(load(Some("ports = [\"8080:80\", \"8080:81\"]\n"), true).is_err());
        assert!(load(Some("ports = [\"8080:80\", \"8081:80\"]\n"), true).is_err());
    }

    #[test]
    fn named_volumes_are_validated_and_deduplicated() {
        let bot = load(
            Some("volumes = [\"data:/app/data\", \"cache:/var/cache/app\"]\n"),
            true,
        )
        .unwrap();
        assert_eq!(bot.volumes.len(), 2);
        assert_eq!(bot.volumes[0].name, "data");
        assert_eq!(bot.volumes[0].target, std::path::PathBuf::from("/app/data"));

        for volumes in [
            "[\"data:relative\"]",
            "[\"../data:/app/data\"]",
            "[\"/host:/app/data\"]",
            "[\"data:/app/../data\"]",
            "[\"data:/app/data\", \"data:/other\"]",
            "[\"data:/app/data\", \"cache:/app/data\"]",
            "[\"bad name:/app/data\"]",
        ] {
            let manifest = format!("volumes = {volumes}\n");
            assert!(load(Some(&manifest), true).is_err(), "volumes {volumes:?}");
        }
    }

    #[test]
    fn config_hash_is_stable_and_changes_with_env() {
        let (temp_a, root_a) = temp_bot(None, true);
        fs::write(root_a.join(".env"), "A=1\n").unwrap();
        let (temp_b, root_b) = temp_bot(None, true);
        fs::write(root_b.join(".env"), "A=1\n").unwrap();
        let a = load_bot(temp_a.path(), "echo-bot").unwrap();
        let b = load_bot(temp_b.path(), "echo-bot").unwrap();
        assert_eq!(a.config_hash, b.config_hash);

        fs::write(root_b.join(".env"), "A=2\n").unwrap();
        let changed = load_bot(temp_b.path(), "echo-bot").unwrap();
        assert_ne!(a.config_hash, changed.config_hash);
    }
}
