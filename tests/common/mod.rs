#![allow(dead_code)]

use anqa::auth::Auth;
use anqa::bots::{BotSource, ResolvedBot, load_bot, slugify};
use anqa::config::Config;
use anqa::docker::{DockerClient, container_name, pull_image, volume_name};
use anqa::state::AppState;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use bollard::Docker;
use bollard::errors::Error as BollardError;
use bollard::exec::{CreateExecOptions, StartExecResults};
use bollard::models::{
    ContainerCreateBody, ContainerInspectResponse, ContainerStateStatusEnum, HostConfig, Mount,
    MountType, RestartPolicy, RestartPolicyNameEnum, VolumeCreateRequest,
};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, RemoveContainerOptionsBuilder, RemoveImageOptionsBuilder,
    RemoveVolumeOptionsBuilder, StartContainerOptionsBuilder,
};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tempfile::TempDir;
use tower::ServiceExt;

pub const SLEEPER_DOCKERFILE: &str = include_str!("../fixtures/sleeper/Dockerfile");
pub const VOLUME_DOCKERFILE: &str = include_str!("../fixtures/volume/Dockerfile");
pub const BROKEN_DOCKERFILE: &str = include_str!("../fixtures/broken/Dockerfile");
const WAIT_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Deserialize)]
pub struct Bot {
    pub dir: String,
    pub name: String,
    pub status: String,
    pub health: Option<String>,
    pub uptime_seconds: u64,
    pub autostart: bool,
    pub busy: bool,
    pub error: Option<String>,
    pub image: Option<String>,
    pub ports: Vec<String>,
    pub volumes: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct BotsResponse {
    bots: Vec<Bot>,
}

pub struct TestEnv {
    pub id: String,
    pub token: String,
    pub bots_dir: PathBuf,
    pub data_dir: PathBuf,
    pub docker: DockerClient,
    pub raw: Docker,
    router: Router,
    _temp: TempDir,
    containers: HashSet<String>,
    volumes: HashSet<String>,
    images: HashSet<String>,
}

impl TestEnv {
    pub async fn new() -> Option<Self> {
        let socket =
            env::var("ANQA_DOCKER_SOCKET").unwrap_or_else(|_| "/var/run/docker.sock".to_string());
        let env = Self::assemble(&socket);
        if let Err(error) = env.docker.ping().await {
            if env::var("ANQA_REQUIRE_DOCKER").as_deref() == Ok("1") {
                panic!("Docker is required for this integration test: {error}");
            }
            eprintln!(
                "skipping real-Docker integration test: Docker is unavailable ({error}); \
                 set ANQA_REQUIRE_DOCKER=1 to fail instead"
            );
            return None;
        }
        Some(env)
    }

    pub fn unavailable() -> Self {
        Self::assemble("/definitely/not/anqa/docker.sock")
    }

    fn assemble(socket: &str) -> Self {
        let id = unique_id();
        let token = format!("test-token-{id}");
        let temp = TempDir::new().expect("temporary test directory");
        let bots_dir = temp.path().join("bots");
        let data_dir = temp.path().join("data");
        fs::create_dir(&bots_dir).expect("create test bots directory");
        fs::create_dir(&data_dir).expect("create test data directory");

        let docker = DockerClient::connect(Path::new(socket));
        let raw = Docker::connect_with_socket(socket, 1800, bollard::API_DEFAULT_VERSION)
            .or_else(|_| {
                Docker::connect_with_http("http://127.0.0.1:1", 1, bollard::API_DEFAULT_VERSION)
            })
            .expect("construct Docker client");
        let config = Config {
            bind_addr: "127.0.0.1:0".parse().expect("test bind address"),
            bots_dir: bots_dir.clone(),
            data_dir: data_dir.clone(),
            docker_socket: PathBuf::from(&socket),
            token_override: Some(token.clone()),
            secure_cookie: false,
        };
        let auth = Auth::new(token.clone(), false);
        let state = AppState::new(config, docker.clone(), auth);
        let router = anqa::api::router(state);

        Self {
            id,
            token,
            bots_dir,
            data_dir,
            docker,
            raw,
            router,
            _temp: temp,
            containers: HashSet::new(),
            volumes: HashSet::new(),
            images: HashSet::new(),
        }
    }

    pub fn write_bot(
        &mut self,
        dir: &str,
        dockerfile: Option<&str>,
        manifest: Option<&str>,
    ) -> ResolvedBot {
        let path = self.bots_dir.join(dir);
        fs::create_dir_all(&path).expect("create bot directory");
        if let Some(contents) = dockerfile {
            fs::write(path.join("Dockerfile"), contents).expect("write Dockerfile");
        }
        if let Some(contents) = manifest {
            fs::write(path.join("bot.toml"), contents).expect("write bot.toml");
        }
        let bot = load_bot(&self.bots_dir, dir).expect("load test bot");
        self.track_bot(&bot);
        bot
    }

    pub fn write_bot_file(&self, dir: &str, name: &str, contents: &str) {
        fs::write(self.bots_dir.join(dir).join(name), contents).expect("write bot file");
    }

    pub fn write_invalid_bot(&mut self, dir: &str, manifest: &str) {
        let path = self.bots_dir.join(dir);
        fs::create_dir_all(&path).expect("create invalid bot directory");
        fs::write(path.join("bot.toml"), manifest).expect("write invalid bot.toml");
        let slug = slugify(dir).expect("test bot directory has a slug");
        self.containers.insert(container_name(&slug));
    }

    fn track_bot(&mut self, bot: &ResolvedBot) {
        self.containers.insert(container_name(&bot.slug));
        if bot.source == BotSource::Dockerfile {
            self.images.insert(bot.image.clone());
        }
        for volume in &bot.volumes {
            self.volumes.insert(volume_name(&bot.slug, &volume.name));
        }
    }

    pub fn track_container(&mut self, name: impl Into<String>) {
        self.containers.insert(name.into());
    }

    pub fn track_volume(&mut self, name: impl Into<String>) {
        self.volumes.insert(name.into());
    }

    pub async fn cleanup(&mut self) {
        for name in std::mem::take(&mut self.containers) {
            let options = RemoveContainerOptionsBuilder::default()
                .force(true)
                .v(false)
                .build();
            let _ = self.raw.remove_container(&name, Some(options)).await;
        }
        for image in std::mem::take(&mut self.images) {
            self.remove_image(&image).await;
        }
        for volume in std::mem::take(&mut self.volumes) {
            let options = RemoveVolumeOptionsBuilder::default().force(true).build();
            let _ = self.raw.remove_volume(&volume, Some(options)).await;
        }
    }

    pub async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> (StatusCode, HeaderMap, Value) {
        let authorization = format!("Bearer {}", self.token);
        self.call_with(method, path, body, Some(("authorization", &authorization)))
            .await
    }

    pub async fn call_no_auth(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> (StatusCode, HeaderMap, Value) {
        self.call_with(method, path, body, None).await
    }

    pub async fn call_with(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        auth: Option<(&str, &str)>,
    ) -> (StatusCode, HeaderMap, Value) {
        let mut builder = Request::builder().method(method).uri(path);
        if let Some((name, value)) = auth {
            builder = builder.header(name, value);
        }
        let body = match body {
            Some(value) => {
                builder = builder.header(header::CONTENT_TYPE, "application/json");
                Body::from(serde_json::to_vec(&value).expect("serialize request body"))
            }
            None => Body::empty(),
        };
        let request = builder.body(body).expect("build request");
        let response = self
            .router
            .clone()
            .oneshot(request)
            .await
            .expect("router response");
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read response body");
        let data = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, headers, data)
    }

    pub async fn get_bots(&self) -> Vec<Bot> {
        let (status, _, body) = self.call(Method::GET, "/api/bots", None).await;
        assert_eq!(status, StatusCode::OK, "GET /api/bots failed: {body}");
        let response: BotsResponse =
            serde_json::from_value(body).expect("parse /api/bots response");
        response.bots
    }

    pub async fn bot(&self, dir: &str) -> Bot {
        self.get_bots()
            .await
            .into_iter()
            .find(|bot| bot.dir == dir)
            .unwrap_or_else(|| panic!("bot {dir} is missing from /api/bots"))
    }

    pub async fn wait_until<F>(&self, dir: &str, mut predicate: F) -> Bot
    where
        F: FnMut(&Bot) -> bool,
    {
        let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
        loop {
            let bot = self.bot(dir).await;
            if predicate(&bot) {
                return bot;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!("timed out waiting for bot {dir}; last state: {bot:?}");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub async fn wait_status(&self, dir: &str, status: &str) -> Bot {
        self.wait_until(dir, |bot| bot.status == status).await
    }

    pub async fn wait_idle(&self, dir: &str) -> Bot {
        self.wait_until(dir, |bot| !bot.busy).await
    }

    pub async fn start(&self, dir: &str) {
        let (status, _, body) = self
            .call(
                Method::POST,
                &format!("/api/bots/{dir}/start"),
                Some(json!({})),
            )
            .await;
        assert_eq!(status, StatusCode::ACCEPTED, "start {dir} failed: {body}");
    }

    pub async fn stop(&self, dir: &str) {
        let (status, _, body) = self
            .call(
                Method::POST,
                &format!("/api/bots/{dir}/stop"),
                Some(json!({})),
            )
            .await;
        assert_eq!(status, StatusCode::ACCEPTED, "stop {dir} failed: {body}");
    }

    pub async fn set_autostart(&self, dir: &str, enabled: bool) {
        let (status, _, body) = self
            .call(
                Method::PUT,
                &format!("/api/bots/{dir}/autostart"),
                Some(json!({ "enabled": enabled })),
            )
            .await;
        assert_eq!(
            status,
            StatusCode::ACCEPTED,
            "autostart {dir} failed: {body}"
        );
    }

    pub async fn inspect(&self, name: &str) -> Option<ContainerInspectResponse> {
        match self.raw.inspect_container(name, None).await {
            Ok(inspect) => Some(inspect),
            Err(BollardError::DockerResponseServerError {
                status_code: 404, ..
            }) => None,
            Err(error) => panic!("inspect {name} failed: {error}"),
        }
    }

    pub async fn container_id(&self, name: &str) -> String {
        self.inspect(name)
            .await
            .unwrap_or_else(|| panic!("container {name} does not exist"))
            .id
            .unwrap_or_else(|| panic!("container {name} has no ID"))
    }

    pub async fn container_status(&self, name: &str) -> ContainerStateStatusEnum {
        self.inspect(name)
            .await
            .unwrap_or_else(|| panic!("container {name} does not exist"))
            .state
            .and_then(|state| state.status)
            .unwrap_or_else(|| panic!("container {name} has no status"))
    }

    pub async fn restart_policy(&self, name: &str) -> Option<RestartPolicyNameEnum> {
        self.inspect(name)
            .await
            .unwrap_or_else(|| panic!("container {name} does not exist"))
            .host_config
            .and_then(|host| host.restart_policy)
            .and_then(|policy| policy.name)
    }

    pub async fn exec_stdout(&self, name: &str, command: &[&str]) -> String {
        let options = CreateExecOptions {
            cmd: Some(command.iter().map(|arg| (*arg).to_string()).collect()),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            ..CreateExecOptions::default()
        };
        let exec = self
            .raw
            .create_exec(name, options)
            .await
            .unwrap_or_else(|error| panic!("create exec in {name} failed: {error}"));
        let mut output = match self
            .raw
            .start_exec(&exec.id, None::<bollard::exec::StartExecOptions>)
            .await
            .unwrap_or_else(|error| panic!("start exec in {name} failed: {error}"))
        {
            StartExecResults::Attached { output, .. } => output,
            StartExecResults::Detached => panic!("exec in {name} unexpectedly detached"),
        };
        let mut bytes = Vec::new();
        while let Some(chunk) = output.next().await {
            bytes.extend_from_slice(chunk.expect("read exec output").as_ref());
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }

    pub async fn ensure_local_image(&self, image: &str) {
        if self.raw.inspect_image(image).await.is_ok() {
            return;
        }
        pull_image(&self.docker, image)
            .await
            .unwrap_or_else(|error| panic!("pull {image} failed: {error}"));
    }

    pub async fn image_exists(&self, image: &str) -> bool {
        self.raw.inspect_image(image).await.is_ok()
    }

    pub async fn remove_image(&self, image: &str) {
        let options = RemoveImageOptionsBuilder::default().force(true).build();
        let _ = self.raw.remove_image(image, Some(options), None).await;
    }

    pub async fn create_raw_volume(&mut self, name: &str) {
        self.raw
            .create_volume(VolumeCreateRequest {
                name: Some(name.to_string()),
                ..VolumeCreateRequest::default()
            })
            .await
            .unwrap_or_else(|error| panic!("create volume {name} failed: {error}"));
        self.track_volume(name);
    }

    pub async fn create_raw_container(
        &mut self,
        name: &str,
        image: &str,
        labels: HashMap<String, String>,
        restart: RestartPolicyNameEnum,
        mount: Option<(&str, &str)>,
        start: bool,
    ) {
        self.ensure_local_image(image).await;
        let mounts = mount.map(|(source, target)| {
            vec![Mount {
                typ: Some(MountType::VOLUME),
                source: Some(source.to_string()),
                target: Some(target.to_string()),
                ..Mount::default()
            }]
        });
        let host_config = HostConfig {
            restart_policy: Some(RestartPolicy {
                name: Some(restart),
                maximum_retry_count: None,
            }),
            mounts,
            ..HostConfig::default()
        };
        let config = ContainerCreateBody {
            image: Some(image.to_string()),
            cmd: Some(vec!["sleep".to_string(), "3600".to_string()]),
            labels: Some(labels),
            host_config: Some(host_config),
            ..ContainerCreateBody::default()
        };
        let options = CreateContainerOptionsBuilder::default().name(name).build();
        self.raw
            .create_container(Some(options), config)
            .await
            .unwrap_or_else(|error| panic!("create container {name} failed: {error}"));
        self.track_container(name);
        if start {
            let options = StartContainerOptionsBuilder::default().build();
            self.raw
                .start_container(name, Some(options))
                .await
                .unwrap_or_else(|error| panic!("start container {name} failed: {error}"));
        }
    }

    pub async fn volume_exists(&self, name: &str) -> bool {
        self.raw.inspect_volume(name).await.is_ok()
    }

    pub async fn pause(&self, name: &str) {
        self.raw
            .pause_container(name)
            .await
            .unwrap_or_else(|error| panic!("pause {name} failed: {error}"));
    }
}

pub fn managed_labels(dir: &str) -> HashMap<String, String> {
    HashMap::from([
        ("anqa.managed".to_string(), "true".to_string()),
        ("anqa.bot".to_string(), dir.to_string()),
    ])
}

fn unique_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after epoch")
        .as_nanos();
    format!(
        "{:x}{:x}{:x}",
        std::process::id(),
        nanos,
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}
