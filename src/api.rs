use axum::body::Body;
use axum::extract::rejection::JsonRejection;
use axum::extract::{DefaultBodyLimit, Path, Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use bollard::models::ContainerInspectResponse;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::path::Path as StdPath;
use std::time::Instant;

use crate::assets;
use crate::bots::{BotErrorCard, BotSource, BotView, Discovery, ResolvedBot, scan, slugify};
use crate::docker::status::{BotStatus, ContainerView, from_inspect};
use crate::docker::{
    DockerError, LifecycleError, container_name, set_autostart, set_restart_policy, start_bot,
    stop_container,
};
use crate::error::ApiError;
use crate::state::{AppState, BotOpSnapshot, BotPhase, OperationGuard};

const BODY_LIMIT: usize = 16 * 1024;

const CSP: &str = "default-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'; img-src 'self' data:; font-src 'self'; style-src 'self'; script-src 'self'; connect-src 'self'";

pub fn router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/api/logout", post(logout))
        .route("/api/bots", get(list_bots))
        .route("/api/bots/{dir}/start", post(start_bot_route))
        .route("/api/bots/{dir}/stop", post(stop_bot_route))
        .route("/api/bots/{dir}/autostart", put(set_autostart_route))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth));

    Router::new()
        .route("/", get(index))
        .route("/assets/{*path}", get(asset))
        .route("/healthz", get(healthz))
        .route("/api/login", post(login))
        .merge(protected)
        .fallback(fallback)
        .layer(middleware::from_fn(log_requests))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .layer(middleware::from_fn(json_content_type))
        .layer(middleware::from_fn(security_headers))
        .with_state(state)
}

#[derive(Deserialize)]
struct LoginRequest {
    token: String,
}

#[derive(Deserialize)]
struct AutostartRequest {
    enabled: bool,
}

struct MutationTarget {
    slug: String,
    bot: Option<ResolvedBot>,
}

async fn list_bots(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    let Discovery { bots, errors } = scan(&state.config.bots_dir).map_err(|error| {
        tracing::warn!(%error, "cannot scan the bots directory");
        ApiError::bots_unavailable()
    })?;

    let containers = state.docker.list_managed().await.map_err(|error| {
        tracing::warn!(%error, "cannot list managed containers");
        ApiError::docker_unavailable()
    })?;

    let mut container_ids = HashMap::new();
    for container in containers {
        let label = container
            .labels
            .as_ref()
            .and_then(|labels| labels.get("anqa.bot"))
            .cloned();
        let id = container.id.clone().or_else(|| {
            container.names.as_ref().and_then(|names| {
                names
                    .first()
                    .map(|name| name.trim_start_matches('/').to_string())
            })
        });
        if let (Some(label), Some(id)) = (label, id) {
            container_ids.entry(label).or_insert(id);
        }
    }

    let mut views = Vec::with_capacity(bots.len() + errors.len());
    for bot in bots {
        let inspect = match container_ids.get(&bot.dir) {
            Some(id) => match state.docker.inspect(id).await {
                Ok(inspect) => (inspect, None),
                Err(error) => (None, Some(format!("Docker inspect failed: {error}"))),
            },
            None => (None, None),
        };
        let snapshot = state.ops.snapshot(&bot.dir);
        views.push(merge_bot_view(
            &bot,
            inspect.0.as_ref(),
            inspect.1,
            snapshot,
        ));
    }
    for error in errors {
        let snapshot = state.ops.snapshot(&error.dir);
        views.push(error_card_view(&error, snapshot));
    }
    views.sort_by(|left, right| left.dir.cmp(&right.dir));

    Ok(Json(json!({ "bots": views })))
}

async fn start_bot_route(
    State(state): State<AppState>,
    Path(dir): Path<String>,
) -> Result<Response, ApiError> {
    let target = resolve_target(&state.config.bots_dir, &dir, true)?;
    let bot = target.bot.expect("a valid manifest was required");
    ensure_docker(&state).await?;
    let guard = begin_operation(&state, &dir, Some(phase_for(&bot)))?;

    let docker = state.docker.clone();
    tokio::spawn(async move {
        let mut guard = guard;
        let result = start_bot(&docker, &bot).await;
        finish_operation(&mut guard, &dir, result);
    });
    Ok(accepted())
}

async fn stop_bot_route(
    State(state): State<AppState>,
    Path(dir): Path<String>,
) -> Result<Response, ApiError> {
    let target = resolve_target(&state.config.bots_dir, &dir, false)?;
    ensure_docker(&state).await?;
    let guard = begin_operation(&state, &dir, Some(BotPhase::Stopping))?;

    let docker = state.docker.clone();
    let name = container_name(&target.slug);
    tokio::spawn(async move {
        let mut guard = guard;
        let result = stop_container(&docker, &name).await;
        finish_operation(&mut guard, &dir, result);
    });
    Ok(accepted())
}

async fn set_autostart_route(
    State(state): State<AppState>,
    Path(dir): Path<String>,
    payload: Result<Json<AutostartRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(AutostartRequest { enabled }) = payload.map_err(json_error)?;
    let target = resolve_target(&state.config.bots_dir, &dir, enabled)?;
    ensure_docker(&state).await?;

    let slug = target.slug;
    let bot = target.bot;
    let phase = if enabled {
        let name = container_name(&slug);
        let exists = state
            .docker
            .inspect(&name)
            .await
            .map_err(map_docker_error)?
            .is_some();
        if exists {
            None
        } else {
            Some(phase_for(
                bot.as_ref()
                    .expect("autostart on requires a valid manifest"),
            ))
        }
    } else {
        None
    };
    let guard = begin_operation(&state, &dir, phase)?;

    let docker = state.docker.clone();
    tokio::spawn(async move {
        let mut guard = guard;
        let result = if enabled {
            match bot {
                Some(bot) => set_autostart(&docker, &bot, true).await,
                None => unreachable!("autostart on was resolved with a manifest"),
            }
        } else {
            set_restart_policy(&docker, &container_name(&slug), false).await
        };
        finish_operation(&mut guard, &dir, result);
    });
    Ok(accepted())
}

fn resolve_target(
    bots_dir: &StdPath,
    dir: &str,
    require_manifest: bool,
) -> Result<MutationTarget, ApiError> {
    let Discovery { bots, errors } = scan(bots_dir).map_err(|error| {
        tracing::warn!(%error, "cannot scan the bots directory");
        ApiError::bots_unavailable()
    })?;

    if let Some(bot) = bots.into_iter().find(|bot| bot.dir == dir) {
        return Ok(MutationTarget {
            slug: bot.slug.clone(),
            bot: Some(bot),
        });
    }

    if let Some(error) = errors.into_iter().find(|error| error.dir == dir) {
        if require_manifest {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid_manifest",
                error.message,
            ));
        }
        let slug = slugify(dir)
            .map_err(|_| ApiError::bad_request("the bot directory cannot be mapped to a slug"))?;
        return Ok(MutationTarget { slug, bot: None });
    }

    Err(ApiError::not_found(
        "the requested bot directory does not exist",
    ))
}

async fn ensure_docker(state: &AppState) -> Result<(), ApiError> {
    state.docker.ping().await.map_err(|error| {
        tracing::warn!(%error, "Docker ping failed");
        ApiError::docker_unavailable()
    })
}

fn begin_operation(
    state: &AppState,
    dir: &str,
    phase: Option<BotPhase>,
) -> Result<OperationGuard, ApiError> {
    state
        .ops
        .begin(dir, phase)
        .ok_or_else(|| ApiError::conflict(format!("an operation is already running for {dir}")))
}

fn finish_operation(guard: &mut OperationGuard, dir: &str, result: Result<(), LifecycleError>) {
    let error = result.err().map(|error| error.to_string());
    if let Some(error) = &error {
        tracing::warn!(bot = dir, %error, "bot operation failed");
    }
    guard.finish(error);
}

fn phase_for(bot: &ResolvedBot) -> BotPhase {
    match bot.source {
        BotSource::Dockerfile => BotPhase::Building,
        BotSource::Image => BotPhase::Starting,
    }
}

fn phase_status(phase: BotPhase) -> BotStatus {
    match phase {
        BotPhase::Building => BotStatus::Building,
        BotPhase::Starting => BotStatus::Starting,
        BotPhase::Stopping => BotStatus::Stopping,
    }
}

fn map_docker_error(error: DockerError) -> ApiError {
    match error {
        DockerError::Unavailable => ApiError::docker_unavailable(),
        error => ApiError::internal(format!("Docker error: {error}")),
    }
}

fn json_error(rejection: JsonRejection) -> ApiError {
    ApiError::new(rejection.status(), "bad_request", rejection.body_text())
}

fn merge_bot_view(
    bot: &ResolvedBot,
    inspect: Option<&ContainerInspectResponse>,
    inspect_error: Option<String>,
    snapshot: BotOpSnapshot,
) -> BotView {
    let container = inspect.map(from_inspect).unwrap_or(ContainerView {
        status: BotStatus::Stopped,
        health: None,
        uptime_seconds: 0,
        autostart: false,
    });
    let mut status = container.status;
    let error = snapshot.error.clone().or(inspect_error);
    if inspect.is_none() && error.is_some() && snapshot.phase.is_none() {
        status = BotStatus::Error;
    }
    if let Some(phase) = snapshot.phase {
        status = phase_status(phase);
    }

    BotView {
        dir: bot.dir.clone(),
        name: bot.name.clone(),
        status,
        health: container.health,
        uptime_seconds: container.uptime_seconds,
        autostart: container.autostart,
        busy: snapshot.busy,
        error,
        image: Some(bot.image.clone()),
        ports: bot.manifest.ports.clone(),
        volumes: bot.manifest.volumes.clone(),
    }
}

fn error_card_view(error: &BotErrorCard, snapshot: BotOpSnapshot) -> BotView {
    BotView {
        dir: error.dir.clone(),
        name: error.dir.clone(),
        status: BotStatus::Error,
        health: None,
        uptime_seconds: 0,
        autostart: false,
        busy: snapshot.busy,
        error: Some(error.message.clone()),
        image: None,
        ports: Vec::new(),
        volumes: Vec::new(),
    }
}

async fn login(
    State(state): State<AppState>,
    payload: Result<Json<LoginRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(LoginRequest { token }) = payload.map_err(json_error)?;

    if !state.auth.verify(&token) {
        return Err(ApiError::invalid_token());
    }

    let mut response = Json(json!({ "ok": true })).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&state.auth.set_cookie())
            .map_err(|_| ApiError::internal("could not create the session cookie"))?,
    );
    Ok(response)
}

async fn logout(State(state): State<AppState>) -> Response {
    let mut response = StatusCode::NO_CONTENT.into_response();
    if let Ok(cookie) = HeaderValue::from_str(&state.auth.clear_cookie()) {
        response.headers_mut().insert(header::SET_COOKIE, cookie);
    }
    response
}

fn accepted() -> Response {
    (StatusCode::ACCEPTED, Json(json!({ "ok": true }))).into_response()
}

async fn healthz(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "version": state.version,
    }))
}

async fn index() -> Response {
    match assets::index() {
        Some(file) => embedded_response("index.html", file),
        None => ApiError::internal("the embedded index is missing").into_response(),
    }
}

async fn asset(Path(path): Path<String>) -> Response {
    match assets::file(&path) {
        Some(file) => embedded_response(&path, file),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn fallback(request: Request) -> Response {
    if request.uri().path().starts_with("/api/") {
        ApiError::not_found("the requested API route does not exist").into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

async fn require_auth(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let Some(token) = crate::auth::session_from_headers(request.headers()) else {
        return ApiError::unauthorized().into_response();
    };
    if !state.auth.verify(&token) {
        return ApiError::invalid_token().into_response();
    }
    next.run(request).await
}

async fn json_content_type(request: Request, next: Next) -> Response {
    let mutation = matches!(
        *request.method(),
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    );
    if mutation && request.uri().path().starts_with("/api") && !has_json_content_type(&request) {
        return ApiError::unsupported_media_type().into_response();
    }
    next.run(request).await
}

fn has_json_content_type(request: &Request) -> bool {
    let Some(value) = request.headers().get(header::CONTENT_TYPE) else {
        return false;
    };
    let Ok(value) = value.to_str() else {
        return false;
    };
    let media_type = value.split(';').next().unwrap_or_default().trim();
    media_type.eq_ignore_ascii_case("application/json")
        || media_type.to_ascii_lowercase().ends_with("+json")
}

async fn security_headers(request: Request, next: Next) -> Response {
    let is_api = request.uri().path().starts_with("/api/");
    let mut response = next.run(request).await;
    let headers = response.headers_mut();

    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), microphone=(), geolocation=(), payment=(), usb=()"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(if is_api { "no-store" } else { "no-cache" }),
    );
    response
}

async fn log_requests(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let started = Instant::now();
    let response = next.run(request).await;
    tracing::info!(
        %method,
        %path,
        status = response.status().as_u16(),
        elapsed_ms = started.elapsed().as_millis(),
        "request"
    );
    response
}

fn embedded_response(path: &str, file: rust_embed::EmbeddedFile) -> Response {
    let content_type = assets::content_type(path);
    let mut response = Response::new(Body::from(file.data.into_owned()));
    if let Ok(content_type) = HeaderValue::from_str(&content_type) {
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, content_type);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::Request;
    use std::path::Path as StdPath;
    use tempfile::TempDir;
    use tower::ServiceExt;

    fn state() -> (AppState, TempDir) {
        let temp = TempDir::new().unwrap();
        let bots_dir = temp.path().join("bots");
        std::fs::create_dir(&bots_dir).unwrap();
        let config = crate::config::Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            bots_dir,
            data_dir: temp.path().join("data"),
            docker_socket: temp.path().join("docker.sock"),
            token_override: None,
            secure_cookie: false,
        };
        let docker =
            crate::docker::DockerClient::connect(StdPath::new("/definitely/not/docker.sock"));
        let state = AppState::new(
            config,
            docker,
            crate::auth::Auth::new("secret".to_string(), false),
        );
        (state, temp)
    }

    fn authed_request(method: Method, uri: &str, body: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, "anqa_session=secret")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    async fn request(
        app: Router,
        request: Request<Body>,
    ) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        let response = app.oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, headers, body.to_vec())
    }

    fn json_request(method: Method, uri: &str, body: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn body_json(body: &[u8]) -> serde_json::Value {
        serde_json::from_slice(body).unwrap()
    }

    #[tokio::test]
    async fn login_sets_a_cookie_and_rejects_a_bad_token() {
        let (state, _temp) = state();
        let app = router(state);

        let (status, headers, body) = request(
            app.clone(),
            json_request(Method::POST, "/api/login", r#"{"token":"secret"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body_json(&body), json!({ "ok": true }));
        assert_eq!(
            headers.get(header::SET_COOKIE).unwrap().to_str().unwrap(),
            "anqa_session=secret; HttpOnly; SameSite=Strict; Path=/; Max-Age=2592000"
        );

        let (status, headers, body) = request(
            app,
            json_request(Method::POST, "/api/login", r#"{"token":"wrong"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body_json(&body)["error"]["code"], "invalid_token");
        assert_eq!(
            headers.get(header::WWW_AUTHENTICATE).unwrap(),
            "Bearer realm=\"anqa\""
        );
    }

    #[tokio::test]
    async fn login_requires_json_content_type_and_a_small_body() {
        let (state, _temp) = state();
        let app = router(state);

        let request_without_type = Request::builder()
            .method(Method::POST)
            .uri("/api/login")
            .body(Body::from(r#"{"token":"secret"}"#))
            .unwrap();
        let (status, _, body) = request(app.clone(), request_without_type).await;
        assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(body_json(&body)["error"]["code"], "unsupported_media_type");

        let large = format!(r#"{{"token":"{}"}}"#, "a".repeat(17 * 1024));
        let (status, _, _) = request(app, json_request(Method::POST, "/api/login", &large)).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn logout_requires_auth_and_clears_the_cookie() {
        let (state, _temp) = state();
        let app = router(state);

        let (status, headers, _) =
            request(app.clone(), json_request(Method::POST, "/api/logout", "")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            headers.get(header::WWW_AUTHENTICATE).unwrap(),
            "Bearer realm=\"anqa\""
        );

        let authenticated = Request::builder()
            .method(Method::POST)
            .uri("/api/logout")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, "anqa_session=secret")
            .body(Body::empty())
            .unwrap();
        let (status, headers, body) = request(app.clone(), authenticated).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert!(body.is_empty());
        assert_eq!(
            headers.get(header::SET_COOKIE).unwrap().to_str().unwrap(),
            "anqa_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0"
        );

        let bearer = Request::builder()
            .method(Method::POST)
            .uri("/api/logout")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, "Bearer secret")
            .body(Body::empty())
            .unwrap();
        let (status, _, _) = request(app, bearer).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn public_health_and_static_assets_have_security_headers() {
        let (state, _temp) = state();
        let app = router(state);

        let (status, headers, body) = request(
            app.clone(),
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body_json(&body),
            json!({ "status": "ok", "version": "0.1.0" })
        );
        assert_eq!(headers.get(header::CACHE_CONTROL).unwrap(), "no-cache");
        assert_eq!(
            headers.get(header::X_CONTENT_TYPE_OPTIONS).unwrap(),
            "nosniff"
        );
        assert_eq!(headers.get(header::X_FRAME_OPTIONS).unwrap(), "DENY");
        assert_eq!(headers.get(header::REFERRER_POLICY).unwrap(), "no-referrer");
        assert!(
            headers
                .get(header::CONTENT_SECURITY_POLICY)
                .unwrap()
                .to_str()
                .unwrap()
                .contains("default-src 'self'")
        );
        assert_eq!(
            headers.get("permissions-policy").unwrap().to_str().unwrap(),
            "camera=(), microphone=(), geolocation=(), payment=(), usb=()"
        );

        let (status, headers, _) = request(
            app.clone(),
            Request::builder()
                .uri("/assets/index.html")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            headers
                .get(header::CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("text/html")
        );

        let (status, _, _) = request(
            app,
            Request::builder()
                .uri("/assets/nope.css")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_responses_are_not_cached() {
        let (state, _temp) = state();
        let app = router(state);
        let (status, headers, _) = request(
            app,
            json_request(Method::POST, "/api/login", r#"{"token":"wrong"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(headers.get(header::CACHE_CONTROL).unwrap(), "no-store");
    }

    fn make_bot(temp: &TempDir) -> ResolvedBot {
        let dir = temp.path().join("bots/echo");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("Dockerfile"), "FROM alpine:3.22\n").unwrap();
        std::fs::write(
            dir.join("bot.toml"),
            "ports = [\"8080:8080\"]\nvolumes = [\"data:/app/data\"]\n",
        )
        .unwrap();
        crate::bots::load_bot(&temp.path().join("bots"), "echo").unwrap()
    }

    fn running_inspect() -> ContainerInspectResponse {
        ContainerInspectResponse {
            image: Some("sha256:image".to_string()),
            state: Some(bollard::models::ContainerState {
                status: Some(bollard::models::ContainerStateStatusEnum::RUNNING),
                ..bollard::models::ContainerState::default()
            }),
            host_config: Some(bollard::models::HostConfig {
                restart_policy: Some(bollard::models::RestartPolicy {
                    name: Some(bollard::models::RestartPolicyNameEnum::NO),
                    ..bollard::models::RestartPolicy::default()
                }),
                ..bollard::models::HostConfig::default()
            }),
            ..ContainerInspectResponse::default()
        }
    }

    #[test]
    fn bot_views_merge_container_status_and_operation_state() {
        let (_state, temp) = state();
        let bot = make_bot(&temp);
        let inspect = running_inspect();

        let running = merge_bot_view(&bot, Some(&inspect), None, BotOpSnapshot::default());
        assert_eq!(running.status, BotStatus::Running);
        assert_eq!(running.image.as_deref(), Some("anqa/echo:latest"));
        assert_eq!(running.ports, vec!["8080:8080".to_string()]);
        assert_eq!(running.volumes, vec!["data:/app/data".to_string()]);
        let json = serde_json::to_value(&running).unwrap();
        assert_eq!(json["image"], "anqa/echo:latest");
        assert_eq!(json["status"], "running");
        assert!(!running.busy);
        assert!(running.error.is_none());

        let busy = merge_bot_view(
            &bot,
            Some(&inspect),
            None,
            BotOpSnapshot {
                busy: true,
                phase: Some(BotPhase::Building),
                error: Some("last failure".to_string()),
            },
        );
        assert_eq!(busy.status, BotStatus::Building);
        assert!(busy.busy);
        assert_eq!(busy.error.as_deref(), Some("last failure"));
        assert_eq!(busy.health, None);

        let inspect_failed = merge_bot_view(
            &bot,
            None,
            Some("Docker inspect failed".to_string()),
            BotOpSnapshot::default(),
        );
        assert_eq!(inspect_failed.status, BotStatus::Error);
        assert!(inspect_failed.error.is_some());
        assert_eq!(inspect_failed.image.as_deref(), Some("anqa/echo:latest"));

        let stopped = merge_bot_view(&bot, None, None, BotOpSnapshot::default());
        assert_eq!(stopped.status, BotStatus::Stopped);
        assert_eq!(stopped.image.as_deref(), Some("anqa/echo:latest"));
    }

    #[test]
    fn error_cards_have_null_images_and_no_manifest_fields() {
        let view = error_card_view(
            &BotErrorCard {
                dir: "bad-bot".to_string(),
                message: "invalid manifest".to_string(),
            },
            BotOpSnapshot::default(),
        );
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["dir"], "bad-bot");
        assert_eq!(json["name"], "bad-bot");
        assert_eq!(json["status"], "error");
        assert_eq!(json["health"], serde_json::Value::Null);
        assert_eq!(json["image"], serde_json::Value::Null);
        assert_eq!(json["ports"], json!([]));
        assert_eq!(json["volumes"], json!([]));
        assert_eq!(json["busy"], false);
        assert_eq!(json["error"], "invalid manifest");
    }

    #[test]
    fn invalid_manifests_resolve_for_stop_but_not_for_start() {
        let (_state, temp) = state();
        let bots = temp.path().join("bots");
        let bad = bots.join("bad");
        std::fs::create_dir(&bad).unwrap();
        std::fs::write(bad.join("bot.toml"), "wat = true\n").unwrap();

        let target = resolve_target(&bots, "bad", false).unwrap();
        assert_eq!(target.slug, "bad");
        assert!(target.bot.is_none());

        let error = resolve_target(&bots, "bad", true).err().unwrap();
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn unknown_directories_are_not_valid_targets() {
        let (_state, temp) = state();
        let bots = temp.path().join("bots");
        let error = resolve_target(&bots, "missing", false).err().unwrap();
        assert_eq!(error.into_response().status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn a_busy_bot_reports_conflict() {
        let (state, _temp) = state();
        let guard = begin_operation(&state, "echo", Some(BotPhase::Starting)).unwrap();
        let error = begin_operation(&state, "echo", None).err().unwrap();
        assert_eq!(error.into_response().status(), StatusCode::CONFLICT);
        drop(guard);
        assert!(begin_operation(&state, "echo", None).is_ok());
    }

    #[tokio::test]
    async fn accepted_mutations_have_the_documented_shape() {
        let response = accepted();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(body_json(&body), json!({ "ok": true }));
    }

    #[tokio::test]
    async fn mutations_resolve_before_docker_and_reject_invalid_manifests() {
        let (state, temp) = state();
        let bad = temp.path().join("bots/bad");
        std::fs::create_dir(&bad).unwrap();
        std::fs::write(bad.join("bot.toml"), "wat = true\n").unwrap();
        let app = router(state);

        let (status, _, body) = request(
            app.clone(),
            authed_request(Method::POST, "/api/bots/bad/start", ""),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body_json(&body)["error"]["code"], "invalid_manifest");

        let (status, _, _) = request(
            app.clone(),
            authed_request(Method::POST, "/api/bots/missing/start", ""),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (status, _, body) = request(app, authed_request(Method::GET, "/api/bots", "")).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body_json(&body)["error"]["code"], "docker_unavailable");
    }
}
