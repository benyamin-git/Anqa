mod common;

use anqa::bots::slugify;
use anqa::docker::{container_name, ensure_orphan_cleanup, volume_name};
use axum::http::{HeaderMap, Method, StatusCode, header};
use bollard::models::{ContainerStateStatusEnum, RestartPolicyNameEnum};
use common::*;
use serde_json::json;
use std::collections::HashMap;

fn cookie_from(headers: &HeaderMap) -> String {
    headers
        .get(header::SET_COOKIE)
        .expect("login sets a cookie")
        .to_str()
        .expect("cookie is ASCII")
        .split(';')
        .next()
        .expect("cookie has a value")
        .to_string()
}

#[tokio::test]
async fn login_logout_cookie_bearer_and_protected_routes() {
    let Some(mut env) = TestEnv::new().await else {
        return;
    };

    let (status, _, _) = env.call_no_auth(Method::GET, "/api/bots", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _, body) = env
        .call_no_auth(
            Method::POST,
            "/api/login",
            Some(json!({ "token": "wrong-token" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "invalid_token");

    let (status, headers, body) = env
        .call_no_auth(
            Method::POST,
            "/api/login",
            Some(json!({ "token": env.token })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], true);
    let cookie = cookie_from(&headers);
    assert!(cookie.starts_with("anqa_session="));
    let set_cookie = headers.get(header::SET_COOKIE).unwrap().to_str().unwrap();
    assert!(set_cookie.contains("HttpOnly"));
    assert!(set_cookie.contains("SameSite=Strict"));

    let (status, _, _) = env
        .call_with(Method::GET, "/api/bots", None, Some(("cookie", &cookie)))
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _, _) = env.call(Method::GET, "/api/bots", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(env.get_bots().await.len(), 0);

    let (status, headers, _) = env
        .call_with(
            Method::POST,
            "/api/logout",
            Some(json!({})),
            Some(("cookie", &cookie)),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let clear_cookie = headers.get(header::SET_COOKIE).unwrap().to_str().unwrap();
    assert!(clear_cookie.contains("Max-Age=0"));

    let (status, _, _) = env.call_with(Method::GET, "/api/bots", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) = env
        .call_with(
            Method::GET,
            "/api/bots",
            None,
            Some(("cookie", "anqa_session=not-a-valid-cookie")),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    env.cleanup().await;
}

#[tokio::test]
async fn build_start_stop_and_restart_after_stop() {
    let Some(mut env) = TestEnv::new().await else {
        return;
    };
    let dir = format!("sleeper-{}", env.id);
    let bot = env.write_bot(&dir, Some(SLEEPER_DOCKERFILE), None);
    let name = container_name(&bot.slug);

    env.start(&dir).await;
    env.wait_status(&dir, "running").await;

    let inspect = env.inspect(&name).await.expect("bot container exists");
    assert_eq!(inspect.name.as_deref(), Some(format!("/{name}").as_str()));
    let config = inspect.config.as_ref().expect("container config");
    let labels = config.labels.as_ref().expect("container labels");
    assert_eq!(labels.get("anqa.managed").map(String::as_str), Some("true"));
    assert_eq!(
        labels.get("anqa.bot").map(String::as_str),
        Some(dir.as_str())
    );
    assert_eq!(labels.get("anqa.config-hash").map(String::len), Some(64));
    assert_eq!(
        labels.get("anqa.version").map(String::as_str),
        Some(env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(
        inspect
            .host_config
            .as_ref()
            .and_then(|host| host.restart_policy.as_ref())
            .and_then(|policy| policy.name),
        Some(RestartPolicyNameEnum::NO)
    );
    assert_eq!(
        inspect.state.as_ref().and_then(|state| state.status),
        Some(ContainerStateStatusEnum::RUNNING)
    );

    env.stop(&dir).await;
    env.wait_status(&dir, "stopped").await;
    assert_eq!(
        env.container_status(&name).await,
        ContainerStateStatusEnum::EXITED
    );

    env.start(&dir).await;
    env.wait_status(&dir, "running").await;
    assert_eq!(
        env.container_status(&name).await,
        ContainerStateStatusEnum::RUNNING
    );

    env.cleanup().await;
}

#[tokio::test]
async fn autostart_toggles_the_restart_policy() {
    let Some(mut env) = TestEnv::new().await else {
        return;
    };
    let dir = format!("autostart-{}", env.id);
    let bot = env.write_bot(&dir, Some(SLEEPER_DOCKERFILE), None);
    let name = container_name(&bot.slug);

    env.start(&dir).await;
    env.wait_status(&dir, "running").await;

    env.set_autostart(&dir, true).await;
    let state = env.wait_until(&dir, |bot| !bot.busy && bot.autostart).await;
    assert!(state.autostart);
    assert_eq!(
        env.restart_policy(&name).await,
        Some(RestartPolicyNameEnum::ALWAYS)
    );

    env.set_autostart(&dir, false).await;
    let state = env
        .wait_until(&dir, |bot| !bot.busy && !bot.autostart)
        .await;
    assert!(!state.autostart);
    assert_eq!(
        env.restart_policy(&name).await,
        Some(RestartPolicyNameEnum::NO)
    );

    env.cleanup().await;
}

#[tokio::test]
async fn autostart_on_creates_a_stopped_container_from_a_dockerfile() {
    let Some(mut env) = TestEnv::new().await else {
        return;
    };
    let dir = format!("autostart-created-{}", env.id);
    let bot = env.write_bot(&dir, Some(SLEEPER_DOCKERFILE), None);
    let name = container_name(&bot.slug);

    env.set_autostart(&dir, true).await;
    let state = env.wait_until(&dir, |bot| !bot.busy && bot.autostart).await;
    assert!(state.autostart);
    assert!(env.image_exists(&bot.image).await);

    let inspect = env.inspect(&name).await.expect("container was created");
    assert_eq!(
        inspect.state.as_ref().and_then(|state| state.status),
        Some(ContainerStateStatusEnum::CREATED)
    );
    assert_eq!(
        env.restart_policy(&name).await,
        Some(RestartPolicyNameEnum::ALWAYS)
    );

    env.set_autostart(&dir, false).await;
    let state = env
        .wait_until(&dir, |bot| !bot.busy && !bot.autostart)
        .await;
    assert!(!state.autostart);
    assert_eq!(
        env.restart_policy(&name).await,
        Some(RestartPolicyNameEnum::NO)
    );

    env.cleanup().await;
}

#[tokio::test]
async fn config_changes_recreate_the_container_and_keep_the_named_volume() {
    let Some(mut env) = TestEnv::new().await else {
        return;
    };
    let dir = format!("volume-{}", env.id);
    let manifest = "name = \"Volume Bot\"\nvolumes = [\"data:/data\"]\n";
    let bot = env.write_bot(&dir, Some(VOLUME_DOCKERFILE), Some(manifest));
    let name = container_name(&bot.slug);

    env.start(&dir).await;
    env.wait_status(&dir, "running").await;
    let first_id = env.container_id(&name).await;
    let log = env.exec_stdout(&name, &["cat", "/data/log"]).await;
    assert_eq!(
        log.lines().filter(|line| !line.trim().is_empty()).count(),
        1,
        "first marker: {log:?}"
    );

    env.stop(&dir).await;
    env.wait_status(&dir, "stopped").await;

    env.write_bot_file(&dir, ".env", "MARKER=two\n");
    env.write_bot_file(
        &dir,
        "bot.toml",
        "name = \"Volume Bot\"\nenv_file = \".env\"\nvolumes = [\"data:/data\"]\n",
    );
    env.start(&dir).await;
    env.wait_status(&dir, "running").await;

    let second_id = env.container_id(&name).await;
    assert_ne!(
        first_id, second_id,
        "config change should recreate the container"
    );
    let log = env.exec_stdout(&name, &["cat", "/data/log"]).await;
    assert_eq!(
        log.lines().filter(|line| !line.trim().is_empty()).count(),
        2,
        "volume marker should survive recreation: {log:?}"
    );

    env.cleanup().await;
}

#[tokio::test]
async fn build_failure_is_reported_and_can_be_fixed() {
    let Some(mut env) = TestEnv::new().await else {
        return;
    };
    let dir = format!("broken-{}", env.id);
    env.write_bot(&dir, Some(BROKEN_DOCKERFILE), None);

    env.start(&dir).await;
    let failed = env
        .wait_until(&dir, |bot| !bot.busy && bot.error.is_some())
        .await;
    assert_eq!(failed.status, "error");
    assert!(failed.image.is_some());
    assert!(
        failed
            .error
            .as_deref()
            .is_some_and(|error| error.contains("build") || error.contains("failed")),
        "unexpected build error: {:?}",
        failed.error
    );

    env.write_bot_file(&dir, "Dockerfile", SLEEPER_DOCKERFILE);
    env.start(&dir).await;
    let fixed = env.wait_status(&dir, "running").await;
    assert_eq!(fixed.error, None);

    env.cleanup().await;
}

#[tokio::test]
async fn invalid_manifest_is_an_error_card_and_lifecycle_still_reaches_managed_containers() {
    let Some(mut env) = TestEnv::new().await else {
        return;
    };
    let valid_dir = format!("valid-{}", env.id);
    let invalid_dir = format!("invalid-{}", env.id);
    let valid = env.write_bot(&valid_dir, Some(SLEEPER_DOCKERFILE), None);
    env.write_invalid_bot(&invalid_dir, "name = \"Invalid Bot\"\n");

    let slug = slugify(&invalid_dir).expect("invalid test directory has a slug");
    let name = container_name(&slug);
    env.create_raw_container(
        &name,
        "alpine:3.22",
        managed_labels(&invalid_dir),
        RestartPolicyNameEnum::ALWAYS,
        None,
        true,
    )
    .await;

    let bots = env.get_bots().await;
    assert_eq!(bots.len(), 2);
    let invalid = bots
        .iter()
        .find(|bot| bot.dir == invalid_dir)
        .expect("invalid bot card");
    assert_eq!(invalid.status, "error");
    assert_eq!(invalid.image, None);
    assert!(invalid.error.is_some());
    let valid_view = bots
        .iter()
        .find(|bot| bot.dir == valid_dir)
        .expect("valid bot card");
    assert_eq!(valid_view.image, Some(valid.image.clone()));

    env.stop(&invalid_dir).await;
    env.wait_idle(&invalid_dir).await;
    assert_eq!(
        env.container_status(&name).await,
        ContainerStateStatusEnum::EXITED
    );

    env.set_autostart(&invalid_dir, false).await;
    env.wait_idle(&invalid_dir).await;
    assert_eq!(
        env.restart_policy(&name).await,
        Some(RestartPolicyNameEnum::NO)
    );

    env.cleanup().await;
}

#[tokio::test]
async fn a_non_managed_container_with_the_bot_name_is_never_touched() {
    let Some(mut env) = TestEnv::new().await else {
        return;
    };
    let dir = format!("conflict-{}", env.id);
    let bot = env.write_bot(&dir, Some(SLEEPER_DOCKERFILE), None);
    let name = container_name(&bot.slug);
    env.create_raw_container(
        &name,
        "alpine:3.22",
        HashMap::new(),
        RestartPolicyNameEnum::ALWAYS,
        None,
        true,
    )
    .await;
    let id = env.container_id(&name).await;

    env.start(&dir).await;
    let failed = env
        .wait_until(&dir, |bot| {
            !bot.busy
                && bot
                    .error
                    .as_deref()
                    .is_some_and(|error| error.contains("not managed"))
        })
        .await;
    assert!(failed.error.is_some());
    assert_eq!(env.container_id(&name).await, id);
    assert_eq!(
        env.container_status(&name).await,
        ContainerStateStatusEnum::RUNNING
    );

    env.stop(&dir).await;
    env.wait_idle(&dir).await;
    assert_eq!(env.container_id(&name).await, id);
    assert_eq!(
        env.container_status(&name).await,
        ContainerStateStatusEnum::RUNNING
    );

    env.set_autostart(&dir, false).await;
    env.wait_idle(&dir).await;
    assert_eq!(env.container_id(&name).await, id);
    assert_eq!(
        env.restart_policy(&name).await,
        Some(RestartPolicyNameEnum::ALWAYS)
    );

    env.cleanup().await;
}

#[tokio::test]
async fn start_recreates_a_paused_managed_container() {
    let Some(mut env) = TestEnv::new().await else {
        return;
    };
    let dir = format!("paused-{}", env.id);
    let bot = env.write_bot(&dir, Some(SLEEPER_DOCKERFILE), None);
    let name = container_name(&bot.slug);

    env.start(&dir).await;
    env.wait_status(&dir, "running").await;
    let first_id = env.container_id(&name).await;
    env.pause(&name).await;
    assert_eq!(
        env.inspect(&name)
            .await
            .and_then(|inspect| inspect.state)
            .and_then(|state| state.paused),
        Some(true)
    );

    env.start(&dir).await;
    env.wait_status(&dir, "running").await;
    let second_id = env.container_id(&name).await;
    assert_ne!(first_id, second_id, "a paused container must be recreated");
    assert_ne!(
        env.inspect(&name)
            .await
            .and_then(|inspect| inspect.state)
            .and_then(|state| state.paused),
        Some(true)
    );

    env.cleanup().await;
}

#[tokio::test]
async fn orphan_cleanup_waits_for_the_second_run_and_keeps_volumes() {
    let Some(mut env) = TestEnv::new().await else {
        return;
    };
    let existing = env
        .docker
        .list_managed()
        .await
        .expect("list managed containers");
    if !existing.is_empty() {
        eprintln!(
            "skipping orphan cleanup test: {} managed container(s) already exist",
            existing.len()
        );
        env.cleanup().await;
        return;
    }

    let orphan_dir = format!("orphan-{}", env.id);
    let slug = slugify(&orphan_dir).expect("orphan test directory has a slug");
    let name = container_name(&slug);
    let volume = volume_name(&slug, "data");
    env.create_raw_volume(&volume).await;
    env.create_raw_container(
        &name,
        "alpine:3.22",
        managed_labels(&orphan_dir),
        RestartPolicyNameEnum::ALWAYS,
        Some((&volume, "/data")),
        true,
    )
    .await;

    let first = ensure_orphan_cleanup(&env.docker, &env.bots_dir, &env.data_dir)
        .await
        .expect("first cleanup run");
    assert!(first.skipped);
    assert!(first.removed.is_empty());
    assert!(env.inspect(&name).await.is_some());
    assert!(env.volume_exists(&volume).await);
    assert!(env.data_dir.join(".anqa-volume").exists());

    let second = ensure_orphan_cleanup(&env.docker, &env.bots_dir, &env.data_dir)
        .await
        .expect("second cleanup run");
    assert!(!second.skipped);
    assert!(second.removed.contains(&name));
    assert!(second.errors.is_empty());
    assert!(env.inspect(&name).await.is_none());
    assert!(env.volume_exists(&volume).await);

    env.cleanup().await;
}

#[tokio::test]
async fn a_prebuilt_image_bot_starts_without_a_dockerfile() {
    let Some(mut env) = TestEnv::new().await else {
        return;
    };
    let dir = format!("image-{}", env.id);
    let manifest =
        "name = \"Prebuilt Bot\"\nimage = \"busybox:1.36.1\"\ncommand = [\"sleep\", \"3600\"]\n";
    let bot = env.write_bot(&dir, None, Some(manifest));
    let name = container_name(&bot.slug);
    let image = "busybox:1.36.1";

    env.remove_image(image).await;
    assert!(!env.image_exists(image).await);

    env.start(&dir).await;
    env.wait_status(&dir, "running").await;
    assert!(env.image_exists(image).await);
    assert_eq!(
        env.container_status(&name).await,
        ContainerStateStatusEnum::RUNNING
    );

    env.cleanup().await;
    env.remove_image(image).await;
}

#[tokio::test]
async fn docker_unavailable_bots_list_returns_503() {
    let env = TestEnv::unavailable();
    let (status, _, body) = env.call(Method::GET, "/api/bots", None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"]["code"], "docker_unavailable");
}
