use anyhow::{Context, bail};
use axum::http::{HeaderMap, header};
use base64::Engine;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use subtle::ConstantTimeEq;

use crate::config::Config;

#[derive(Clone)]
pub struct Auth {
    token: String,
    secure_cookie: bool,
}

impl Auth {
    pub fn load(config: &Config) -> anyhow::Result<Self> {
        fs::create_dir_all(&config.data_dir).with_context(|| {
            format!("cannot create data directory {}", config.data_dir.display())
        })?;
        ensure_writable(&config.data_dir)?;

        if let Some(token) = &config.token_override {
            if token.trim().is_empty() {
                bail!("ANQA_TOKEN must not be empty or whitespace");
            }
            return Ok(Self::new(token.clone(), config.secure_cookie));
        }

        let path = config.data_dir.join("token");
        match fs::read_to_string(&path) {
            Ok(contents) => Self::from_file_contents(&contents, config.secure_cookie),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Self::generate(&config.data_dir, config.secure_cookie)
            }
            Err(error) => {
                Err(error).with_context(|| format!("cannot read token file {}", path.display()))
            }
        }
    }

    pub fn new(token: String, secure_cookie: bool) -> Self {
        Self {
            token,
            secure_cookie,
        }
    }

    pub fn verify(&self, candidate: &str) -> bool {
        if self.token.len() != candidate.len() {
            return false;
        }
        self.token.as_bytes().ct_eq(candidate.as_bytes()).into()
    }

    pub fn set_cookie(&self) -> String {
        format!(
            "anqa_session={}; HttpOnly; SameSite=Strict; Path=/; Max-Age=2592000{}",
            self.token,
            self.secure_suffix()
        )
    }

    pub fn clear_cookie(&self) -> String {
        format!(
            "anqa_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0{}",
            self.secure_suffix()
        )
    }

    fn secure_suffix(&self) -> &'static str {
        if self.secure_cookie { "; Secure" } else { "" }
    }

    fn from_file_contents(contents: &str, secure_cookie: bool) -> anyhow::Result<Self> {
        let token = contents.trim();
        if token.is_empty() {
            bail!("token file is empty; delete it to generate a new token or set ANQA_TOKEN");
        }
        Ok(Self::new(token.to_string(), secure_cookie))
    }

    fn generate(data_dir: &Path, secure_cookie: bool) -> anyhow::Result<Self> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).context("cannot read random bytes for the dashboard token")?;
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        let path: PathBuf = data_dir.join("token");

        let mut options = OpenOptions::new();
        options.write(true).create_new(true).mode(0o600);
        match options.open(&path) {
            Ok(mut file) => {
                if let Err(error) = file
                    .write_all(token.as_bytes())
                    .and_then(|_| file.sync_all())
                {
                    let _ = fs::remove_file(&path);
                    return Err(error)
                        .with_context(|| format!("cannot write token file {}", path.display()));
                }
                tracing::info!(token = %token, "generated dashboard token");
                Ok(Self::new(token, secure_cookie))
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let contents = fs::read_to_string(&path)
                    .with_context(|| format!("cannot read token file {}", path.display()))?;
                Self::from_file_contents(&contents, secure_cookie)
            }
            Err(error) => {
                Err(error).with_context(|| format!("cannot create token file {}", path.display()))
            }
        }
    }
}

fn ensure_writable(data_dir: &Path) -> anyhow::Result<()> {
    let probe = data_dir.join(".anqa-write-test");
    let result = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&probe);
    match result {
        Ok(file) => {
            drop(file);
            let _ = fs::remove_file(probe);
            Ok(())
        }
        Err(error) => Err(error)
            .with_context(|| format!("data directory {} is not writable", data_dir.display())),
    }
}

impl fmt::Debug for Auth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Auth")
            .field("token", &"<redacted>")
            .field("secure_cookie", &self.secure_cookie)
            .finish()
    }
}

pub fn session_from_headers(headers: &HeaderMap) -> Option<String> {
    for value in headers.get_all(header::COOKIE) {
        let Ok(cookies) = value.to_str() else {
            continue;
        };
        for cookie in cookies.split(';') {
            let Some((name, value)) = cookie.trim().split_once('=') else {
                continue;
            };
            if name.trim() == "anqa_session" && !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }

    for value in headers.get_all(header::AUTHORIZATION) {
        let Ok(authorization) = value.to_str() else {
            continue;
        };
        let Some((scheme, token)) = authorization.trim().split_once(' ') else {
            continue;
        };
        if scheme.eq_ignore_ascii_case("bearer") && !token.is_empty() {
            return Some(token.to_string());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue, header};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use tempfile::TempDir;

    fn config(data_dir: &Path, token_override: Option<&str>, secure_cookie: bool) -> Config {
        Config {
            bind_addr: "0.0.0.0:7409".parse().unwrap(),
            bots_dir: data_dir.join("bots"),
            data_dir: data_dir.to_path_buf(),
            docker_socket: data_dir.join("docker.sock"),
            token_override: token_override.map(str::to_string),
            secure_cookie,
        }
    }

    #[test]
    fn generated_token_is_43_url_safe_characters_and_mode_0600() {
        let temp = TempDir::new().unwrap();
        let auth = Auth::load(&config(temp.path(), None, false)).unwrap();
        let path = temp.path().join("token");
        let token = fs::read_to_string(&path).unwrap();
        assert_eq!(token.len(), 43);
        assert!(
            token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        );
        assert!(auth.verify(&token));
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        let reloaded = Auth::load(&config(temp.path(), None, false)).unwrap();
        assert!(reloaded.verify(&token));
    }

    #[test]
    fn an_existing_token_is_reused_unchanged() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("token");
        fs::write(&path, "existing-token").unwrap();
        let auth = Auth::load(&config(temp.path(), None, false)).unwrap();
        assert!(auth.verify("existing-token"));
        assert_eq!(fs::read_to_string(path).unwrap(), "existing-token");
    }

    #[test]
    fn env_override_wins_and_is_not_written() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("token");
        fs::write(&path, "file-token").unwrap();
        let auth = Auth::load(&config(temp.path(), Some("env-token"), false)).unwrap();
        assert!(auth.verify("env-token"));
        assert!(!auth.verify("file-token"));
        assert_eq!(fs::read_to_string(path).unwrap(), "file-token");
    }

    #[test]
    fn empty_or_whitespace_token_file_is_an_error() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("token"), " \n\t").unwrap();
        let error = Auth::load(&config(temp.path(), None, false)).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("delete") || message.contains("rotate"));
    }

    #[test]
    fn verify_requires_an_exact_match() {
        let auth = Auth::new("correct-token".to_string(), false);
        assert!(auth.verify("correct-token"));
        assert!(!auth.verify("correct-tokeN"));
        assert!(!auth.verify("correct-token "));
        assert!(!auth.verify(""));
    }

    #[test]
    fn cookie_attributes_depend_on_secure_setting() {
        let insecure = Auth::new("token".to_string(), false);
        assert_eq!(
            insecure.set_cookie(),
            "anqa_session=token; HttpOnly; SameSite=Strict; Path=/; Max-Age=2592000"
        );
        assert_eq!(
            insecure.clear_cookie(),
            "anqa_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0"
        );

        let secure = Auth::new("token".to_string(), true);
        assert!(secure.set_cookie().ends_with("; Secure"));
        assert!(secure.clear_cookie().ends_with("; Secure"));
    }

    #[test]
    fn session_cookie_parser_handles_multiple_cookies_and_headers() {
        let mut headers = HeaderMap::new();
        headers.append(
            header::COOKIE,
            HeaderValue::from_static("first=1; anqa_session=abc; last=2"),
        );
        assert_eq!(session_from_headers(&headers).as_deref(), Some("abc"));

        headers.clear();
        headers.append(header::COOKIE, HeaderValue::from_static("first=1"));
        headers.append(header::COOKIE, HeaderValue::from_static("anqa_session=def"));
        assert_eq!(session_from_headers(&headers).as_deref(), Some("def"));

        headers.clear();
        headers.append(header::COOKIE, HeaderValue::from_static("first=1; last=2"));
        assert_eq!(session_from_headers(&headers), None);
    }

    #[test]
    fn bearer_authorization_is_accepted() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer bearer-token"),
        );
        assert_eq!(
            session_from_headers(&headers).as_deref(),
            Some("bearer-token")
        );
    }

    #[test]
    fn debug_output_redacts_the_token() {
        let debug = format!("{:?}", Auth::new("secret".to_string(), false));
        assert!(debug.contains("Auth"));
        assert!(!debug.contains("secret"));
    }
}
