use anqa::bots::load_bot;
use anqa::docker::{DockerClient, build_dockerfile};
use bollard::Docker;
use bollard::query_parameters::RemoveImageOptionsBuilder;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

#[tokio::test]
#[ignore]
async fn buildkit_builds_a_temporary_bot_image() {
    if std::env::var("ANQA_REQUIRE_DOCKER").as_deref() != Ok("1") {
        eprintln!("set ANQA_REQUIRE_DOCKER=1 to run the BuildKit spike");
        return;
    }

    let temp = TempDir::new().unwrap();
    let bot_dir = temp.path().join("spike");
    fs::create_dir(&bot_dir).unwrap();
    fs::write(
        bot_dir.join("Dockerfile"),
        "FROM alpine:3.22\nCMD [\"true\"]\n",
    )
    .unwrap();

    let bot = load_bot(temp.path(), "spike").unwrap();
    let docker = DockerClient::connect(Path::new("/var/run/docker.sock"))
        .try_negotiate()
        .await;
    let image_id = build_dockerfile(&docker, &bot)
        .await
        .expect("BuildKit build should succeed");
    assert!(image_id.starts_with("sha256:"));

    let raw =
        Docker::connect_with_socket("/var/run/docker.sock", 1800, bollard::API_DEFAULT_VERSION)
            .unwrap();
    let inspect = raw
        .inspect_image(&bot.image)
        .await
        .expect("built image should exist");
    assert_eq!(inspect.id.as_deref(), Some(image_id.as_str()));

    let options = RemoveImageOptionsBuilder::default().force(true).build();
    raw.remove_image(&bot.image, Some(options), None)
        .await
        .expect("the spike image should be removed");
}
