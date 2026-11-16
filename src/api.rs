use axum::body::Body;
use axum::extract::rejection::JsonRejection;
use axum::extract::{DefaultBodyLimit, Path, Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use std::time::Instant;

use crate::assets;
use crate::error::ApiError;
use crate::state::AppState;

const BODY_LIMIT: usize = 16 * 1024;

const CSP: &str = "default-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'; img-src 'self' data:; font-src 'self'; style-src 'self'; script-src 'self'; connect-src 'self'";

pub fn router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/api/logout", post(logout))
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

async fn login(
    State(state): State<AppState>,
    payload: Result<Json<LoginRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(LoginRequest { token }) = payload.map_err(|rejection| {
        ApiError::new(rejection.status(), "bad_request", rejection.body_text())
    })?;

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
        let config = crate::config::Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            bots_dir: temp.path().join("bots"),
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
}
