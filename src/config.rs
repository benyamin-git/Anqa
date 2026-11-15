use anyhow::{Context, bail};
use std::collections::HashMap;
use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Clone)]
pub struct Config {
    pub bind_addr: SocketAddr,
    pub bots_dir: PathBuf,
    pub data_dir: PathBuf,
    pub docker_socket: PathBuf,
    pub token_override: Option<String>,
    pub secure_cookie: bool,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Self::from_map(std::env::vars().collect())
    }

    fn from_map(vars: HashMap<String, String>) -> anyhow::Result<Self> {
        let port = match vars.get("ANQA_PORT") {
            Some(value) => value
                .parse::<u16>()
                .with_context(|| format!("ANQA_PORT must be a number, got {value:?}"))?,
            None => 7409,
        };
        if port == 0 {
            bail!("ANQA_PORT must be between 1 and 65535");
        }

        let token_override = vars
            .get("ANQA_TOKEN")
            .filter(|value| !value.is_empty())
            .cloned();
        let secure_cookie = matches!(
            vars.get("ANQA_SECURE_COOKIE")
                .map(|value| value.to_ascii_lowercase())
                .as_deref(),
            Some("1" | "true")
        );

        Ok(Self {
            bind_addr: SocketAddr::from(([0, 0, 0, 0], port)),
            bots_dir: path_var(&vars, "ANQA_BOTS_DIR", "/bots"),
            data_dir: path_var(&vars, "ANQA_DATA_DIR", "/data"),
            docker_socket: path_var(&vars, "ANQA_DOCKER_SOCKET", "/var/run/docker.sock"),
            token_override,
            secure_cookie,
        })
    }
}

fn path_var(vars: &HashMap<String, String>, key: &str, default: &str) -> PathBuf {
    vars.get(key)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(default))
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("bind_addr", &self.bind_addr)
            .field("bots_dir", &self.bots_dir)
            .field("data_dir", &self.data_dir)
            .field("docker_socket", &self.docker_socket)
            .field(
                "token_override",
                &self.token_override.as_ref().map(|_| "<redacted>"),
            )
            .field("secure_cookie", &self.secure_cookie)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(items: &[(&str, &str)]) -> HashMap<String, String> {
        items
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    #[test]
    fn defaults_are_the_documented_container_paths() {
        let config = Config::from_map(map(&[])).unwrap();
        assert_eq!(config.bind_addr, "0.0.0.0:7409".parse().unwrap());
        assert_eq!(config.bots_dir, PathBuf::from("/bots"));
        assert_eq!(config.data_dir, PathBuf::from("/data"));
        assert_eq!(config.docker_socket, PathBuf::from("/var/run/docker.sock"));
        assert_eq!(config.token_override, None);
        assert!(!config.secure_cookie);
    }

    #[test]
    fn custom_port_paths_and_values_are_parsed() {
        let config = Config::from_map(map(&[
            ("ANQA_PORT", "8123"),
            ("ANQA_BOTS_DIR", "/srv/bots"),
            ("ANQA_DATA_DIR", "/srv/data"),
            ("ANQA_DOCKER_SOCKET", "/run/docker.sock"),
            ("ANQA_TOKEN", "secret"),
        ]))
        .unwrap();
        assert_eq!(config.bind_addr, "0.0.0.0:8123".parse().unwrap());
        assert_eq!(config.bots_dir, PathBuf::from("/srv/bots"));
        assert_eq!(config.data_dir, PathBuf::from("/srv/data"));
        assert_eq!(config.docker_socket, PathBuf::from("/run/docker.sock"));
        assert_eq!(config.token_override.as_deref(), Some("secret"));
    }

    #[test]
    fn invalid_port_is_an_error() {
        assert!(Config::from_map(map(&[("ANQA_PORT", "0")])).is_err());
        assert!(Config::from_map(map(&[("ANQA_PORT", "65536")])).is_err());
        assert!(Config::from_map(map(&[("ANQA_PORT", "abc")])).is_err());
    }

    #[test]
    fn secure_cookie_accepts_1_and_true_only() {
        for (value, expected) in [
            ("1", true),
            ("true", true),
            ("TRUE", true),
            ("0", false),
            ("false", false),
            ("yes", false),
        ] {
            let config = Config::from_map(map(&[("ANQA_SECURE_COOKIE", value)])).unwrap();
            assert_eq!(config.secure_cookie, expected, "value {value:?}");
        }
    }

    #[test]
    fn token_override_is_used_only_when_non_empty() {
        let empty = Config::from_map(map(&[("ANQA_TOKEN", "")])).unwrap();
        assert_eq!(empty.token_override, None);
        let set = Config::from_map(map(&[("ANQA_TOKEN", "secret")])).unwrap();
        assert_eq!(set.token_override.as_deref(), Some("secret"));
    }

    #[test]
    fn debug_output_redacts_the_token() {
        let config = Config::from_map(map(&[("ANQA_TOKEN", "secret")])).unwrap();
        let debug = format!("{config:?}");
        assert!(debug.contains("token_override"));
        assert!(!debug.contains("secret"));
    }
}
