use bollard::models::{
    ContainerCreateBody, ContainerInspectResponse, ContainerStateStatusEnum, ContainerUpdateBody,
    HostConfig, HostConfigLogConfig, Mount, MountType, PortBinding, PortMap, RestartPolicy,
    RestartPolicyNameEnum, VolumeCreateRequest,
};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, RemoveContainerOptionsBuilder, StartContainerOptionsBuilder,
    StopContainerOptionsBuilder,
};
use std::collections::HashMap;

use super::build::{BuildError, ensure_image};
use super::{DockerClient, DockerError, map_bollard_error};
use crate::bots::{PortProtocol, ResolvedBot, VolumeSpec};

const CONTAINER_PREFIX: &str = "anqa-";
const VOLUME_PREFIX: &str = "anqa-";
const MANAGED_LABEL: &str = "anqa.managed";
const BOT_LABEL: &str = "anqa.bot";
const CONFIG_HASH_LABEL: &str = "anqa.config-hash";
const VERSION_LABEL: &str = "anqa.version";
const LOG_MAX_SIZE: &str = "10m";
const LOG_MAX_FILE: &str = "3";

#[derive(Debug, thiserror::Error)]
pub enum LifecycleError {
    #[error("Docker error: {0}")]
    Docker(#[from] DockerError),
    #[error("build error: {0}")]
    Build(#[from] BuildError),
    #[error("container {name} is not managed by Anqa")]
    NameConflict { name: String },
}

pub async fn start_bot(docker: &DockerClient, bot: &ResolvedBot) -> Result<(), LifecycleError> {
    let name = container_name(&bot.slug);
    let existing = docker.inspect(&name).await?;

    if let Some(inspect) = existing.as_ref() {
        require_managed(&name, inspect)?;
        if is_running_and_not_paused(inspect) {
            return Ok(());
        }
    }

    let image_id = ensure_image(docker, bot).await?;
    ensure_volumes(docker, bot).await?;

    let autostart = existing.as_ref().is_some_and(restart_policy_enabled);
    ensure_container(docker, bot, &image_id, autostart).await?;
    start_container(docker, &name).await
}

pub async fn stop_bot(docker: &DockerClient, bot: &ResolvedBot) -> Result<(), LifecycleError> {
    stop_container(docker, &container_name(&bot.slug)).await
}

pub async fn stop_container(
    docker: &DockerClient,
    container_name: &str,
) -> Result<(), LifecycleError> {
    let Some(inspect) = docker.inspect(container_name).await? else {
        return Ok(());
    };
    require_managed(container_name, &inspect)?;

    if !is_stoppable(&inspect) {
        return Ok(());
    }

    let client = docker.client().await?;
    let options = StopContainerOptionsBuilder::default().t(10).build();
    client
        .stop_container(container_name, Some(options))
        .await
        .map_err(map_bollard_error)?;
    Ok(())
}

pub async fn set_autostart(
    docker: &DockerClient,
    bot: &ResolvedBot,
    enabled: bool,
) -> Result<(), LifecycleError> {
    let name = container_name(&bot.slug);
    if docker.inspect(&name).await?.is_some() {
        return set_restart_policy(docker, &name, enabled).await;
    }
    if !enabled {
        return Ok(());
    }

    let image_id = ensure_image(docker, bot).await?;
    ensure_volumes(docker, bot).await?;
    ensure_container(docker, bot, &image_id, true).await?;
    set_restart_policy(docker, &name, true).await
}

pub async fn set_restart_policy(
    docker: &DockerClient,
    container_name: &str,
    enabled: bool,
) -> Result<(), LifecycleError> {
    let Some(inspect) = docker.inspect(container_name).await? else {
        return Ok(());
    };
    require_managed(container_name, &inspect)?;

    let client = docker.client().await?;
    client
        .update_container(
            container_name,
            ContainerUpdateBody {
                restart_policy: Some(restart_policy(enabled)),
                ..ContainerUpdateBody::default()
            },
        )
        .await
        .map_err(map_bollard_error)?;
    Ok(())
}

async fn ensure_container(
    docker: &DockerClient,
    bot: &ResolvedBot,
    image_id: &str,
    autostart: bool,
) -> Result<(), LifecycleError> {
    let name = container_name(&bot.slug);

    if let Some(inspect) = docker.inspect(&name).await? {
        require_managed(&name, &inspect)?;
        if can_reuse(&inspect, bot, image_id) {
            return Ok(());
        }

        if is_stoppable(&inspect) {
            let client = docker.client().await?;
            let options = StopContainerOptionsBuilder::default().t(10).build();
            client
                .stop_container(&name, Some(options))
                .await
                .map_err(map_bollard_error)?;
        }

        let client = docker.client().await?;
        let options = RemoveContainerOptionsBuilder::default()
            .force(true)
            .v(false)
            .build();
        client
            .remove_container(&name, Some(options))
            .await
            .map_err(map_bollard_error)?;
    }

    create_container(docker, bot, image_id, autostart).await
}

async fn ensure_volumes(docker: &DockerClient, bot: &ResolvedBot) -> Result<(), LifecycleError> {
    let client = docker.client().await?;
    for volume in &bot.volumes {
        client
            .create_volume(volume_request(bot, volume))
            .await
            .map_err(map_bollard_error)?;
    }
    Ok(())
}

async fn create_container(
    docker: &DockerClient,
    bot: &ResolvedBot,
    image_id: &str,
    autostart: bool,
) -> Result<(), LifecycleError> {
    let name = container_name(&bot.slug);
    let client = docker.client().await?;
    let options = CreateContainerOptionsBuilder::default().name(&name).build();
    client
        .create_container(Some(options), container_config(bot, image_id, autostart))
        .await
        .map(|_| ())
        .map_err(|error| map_create_error(&name, map_bollard_error(error)))
}

async fn start_container(
    docker: &DockerClient,
    container_name: &str,
) -> Result<(), LifecycleError> {
    let client = docker.client().await?;
    let options = StartContainerOptionsBuilder::default().build();
    client
        .start_container(container_name, Some(options))
        .await
        .map_err(map_bollard_error)?;
    Ok(())
}

fn container_config(bot: &ResolvedBot, image_id: &str, autostart: bool) -> ContainerCreateBody {
    let exposed_ports = exposed_ports(bot);
    ContainerCreateBody {
        image: Some(image_id.to_string()),
        env: Some(environment(bot)),
        cmd: bot.manifest.command.clone(),
        exposed_ports: Some(exposed_ports),
        labels: Some(labels(bot)),
        host_config: Some(host_config(bot, autostart)),
        ..ContainerCreateBody::default()
    }
}

fn host_config(bot: &ResolvedBot, autostart: bool) -> HostConfig {
    HostConfig {
        mounts: Some(mounts(bot)),
        port_bindings: Some(port_bindings(bot)),
        restart_policy: Some(restart_policy(autostart)),
        log_config: Some(HostConfigLogConfig {
            typ: Some("json-file".to_string()),
            config: Some(HashMap::from([
                ("max-size".to_string(), LOG_MAX_SIZE.to_string()),
                ("max-file".to_string(), LOG_MAX_FILE.to_string()),
            ])),
        }),
        auto_remove: Some(false),
        ..HostConfig::default()
    }
}

fn labels(bot: &ResolvedBot) -> HashMap<String, String> {
    HashMap::from([
        (MANAGED_LABEL.to_string(), "true".to_string()),
        (BOT_LABEL.to_string(), bot.dir.clone()),
        (CONFIG_HASH_LABEL.to_string(), bot.config_hash.clone()),
        (
            VERSION_LABEL.to_string(),
            env!("CARGO_PKG_VERSION").to_string(),
        ),
    ])
}

fn environment(bot: &ResolvedBot) -> Vec<String> {
    let mut environment: Vec<String> = bot
        .env
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    environment.sort();
    environment
}

fn exposed_ports(bot: &ResolvedBot) -> Vec<String> {
    bot.ports
        .iter()
        .map(|port| port_key(port.container_port, port.protocol))
        .collect()
}

fn port_bindings(bot: &ResolvedBot) -> PortMap {
    bot.ports
        .iter()
        .map(|port| {
            (
                port_key(port.container_port, port.protocol),
                Some(vec![PortBinding {
                    host_ip: port.host_ip.clone(),
                    host_port: Some(port.host_port.to_string()),
                }]),
            )
        })
        .collect()
}

fn port_key(container_port: u16, protocol: PortProtocol) -> String {
    let protocol = match protocol {
        PortProtocol::Tcp => "tcp",
        PortProtocol::Udp => "udp",
    };
    format!("{container_port}/{protocol}")
}

fn mounts(bot: &ResolvedBot) -> Vec<Mount> {
    bot.volumes
        .iter()
        .map(|volume| Mount {
            typ: Some(MountType::VOLUME),
            source: Some(volume_name(&bot.slug, &volume.name)),
            target: Some(volume.target.to_string_lossy().into_owned()),
            ..Mount::default()
        })
        .collect()
}

fn volume_request(bot: &ResolvedBot, volume: &VolumeSpec) -> VolumeCreateRequest {
    VolumeCreateRequest {
        name: Some(volume_name(&bot.slug, &volume.name)),
        labels: Some(HashMap::from([
            (MANAGED_LABEL.to_string(), "true".to_string()),
            (BOT_LABEL.to_string(), bot.dir.clone()),
        ])),
        ..VolumeCreateRequest::default()
    }
}

fn restart_policy(enabled: bool) -> RestartPolicy {
    RestartPolicy {
        name: Some(if enabled {
            RestartPolicyNameEnum::ALWAYS
        } else {
            RestartPolicyNameEnum::NO
        }),
        maximum_retry_count: None,
    }
}

fn restart_policy_enabled(inspect: &ContainerInspectResponse) -> bool {
    inspect
        .host_config
        .as_ref()
        .and_then(|config| config.restart_policy.as_ref())
        .and_then(|policy| policy.name)
        == Some(RestartPolicyNameEnum::ALWAYS)
}

fn is_running_and_not_paused(inspect: &ContainerInspectResponse) -> bool {
    let Some(state) = inspect.state.as_ref() else {
        return false;
    };
    state.paused != Some(true) && state.status == Some(ContainerStateStatusEnum::RUNNING)
}

fn is_stoppable(inspect: &ContainerInspectResponse) -> bool {
    let Some(state) = inspect.state.as_ref() else {
        return false;
    };
    state.paused != Some(true)
        && matches!(
            state.status,
            Some(ContainerStateStatusEnum::RUNNING | ContainerStateStatusEnum::RESTARTING)
        )
}

fn can_reuse(inspect: &ContainerInspectResponse, bot: &ResolvedBot, image_id: &str) -> bool {
    is_managed(inspect)
        && label(inspect, CONFIG_HASH_LABEL) == Some(bot.config_hash.as_str())
        && inspect.image.as_deref() == Some(image_id)
        && has_reusable_state(inspect)
}

fn has_reusable_state(inspect: &ContainerInspectResponse) -> bool {
    let Some(state) = inspect.state.as_ref() else {
        return false;
    };
    state.paused != Some(true)
        && state.dead != Some(true)
        && matches!(
            state.status,
            Some(
                ContainerStateStatusEnum::CREATED
                    | ContainerStateStatusEnum::EXITED
                    | ContainerStateStatusEnum::RUNNING
            )
        )
}

fn require_managed(name: &str, inspect: &ContainerInspectResponse) -> Result<(), LifecycleError> {
    if is_managed(inspect) {
        Ok(())
    } else {
        Err(LifecycleError::NameConflict {
            name: name.to_string(),
        })
    }
}

fn is_managed(inspect: &ContainerInspectResponse) -> bool {
    label(inspect, MANAGED_LABEL) == Some("true")
}

fn label<'a>(inspect: &'a ContainerInspectResponse, key: &str) -> Option<&'a str> {
    inspect
        .config
        .as_ref()
        .and_then(|config| config.labels.as_ref())
        .and_then(|labels| labels.get(key))
        .map(String::as_str)
}

fn map_create_error(name: &str, error: DockerError) -> LifecycleError {
    match error {
        DockerError::Conflict { .. } => LifecycleError::NameConflict {
            name: name.to_string(),
        },
        error => LifecycleError::Docker(error),
    }
}

pub fn container_name(slug: &str) -> String {
    format!("{CONTAINER_PREFIX}{slug}")
}

pub fn volume_name(slug: &str, volume: &str) -> String {
    format!("{VOLUME_PREFIX}{slug}-{volume}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bots::{BotSource, Manifest, PortSpec};
    use bollard::models::{
        ContainerConfig, ContainerState, ContainerStateStatusEnum, HealthStatusEnum,
        HostConfigLogConfig,
    };
    use std::path::PathBuf;

    fn bot() -> ResolvedBot {
        ResolvedBot {
            dir: "echo-bot".to_string(),
            slug: "echo-bot".to_string(),
            name: "Echo Bot".to_string(),
            image: "anqa/echo-bot:latest".to_string(),
            source: BotSource::Dockerfile,
            manifest: Manifest {
                command: Some(vec!["run".to_string(), "--prod".to_string()]),
                ..Manifest::default()
            },
            env: vec![
                ("B".to_string(), "2".to_string()),
                ("A".to_string(), "1".to_string()),
            ],
            ports: Vec::new(),
            volumes: Vec::new(),
            config_hash: "a".repeat(64),
            root: PathBuf::from("/bots"),
        }
    }

    fn inspect_with(
        status: ContainerStateStatusEnum,
        paused: bool,
        labels: HashMap<String, String>,
        image: &str,
    ) -> ContainerInspectResponse {
        ContainerInspectResponse {
            image: Some(image.to_string()),
            state: Some(ContainerState {
                status: Some(status),
                paused: Some(paused),
                ..ContainerState::default()
            }),
            config: Some(ContainerConfig {
                labels: Some(labels),
                ..ContainerConfig::default()
            }),
            ..ContainerInspectResponse::default()
        }
    }

    fn managed_labels(bot: &ResolvedBot) -> HashMap<String, String> {
        labels(bot)
    }

    #[test]
    fn resource_names_are_prefixed_and_stable() {
        assert_eq!(container_name("echo-bot"), "anqa-echo-bot");
        assert_eq!(volume_name("echo-bot", "data"), "anqa-echo-bot-data");
    }

    #[test]
    fn restart_policies_map_to_always_and_no() {
        assert_eq!(
            restart_policy(true).name,
            Some(RestartPolicyNameEnum::ALWAYS)
        );
        assert_eq!(restart_policy(false).name, Some(RestartPolicyNameEnum::NO));
        assert_eq!(restart_policy(true).maximum_retry_count, None);
    }

    #[test]
    fn config_hash_and_image_changes_force_recreation() {
        let mut bot = bot();
        let image = "sha256:image";
        let inspect = inspect_with(
            ContainerStateStatusEnum::EXITED,
            false,
            managed_labels(&bot),
            image,
        );
        assert!(can_reuse(&inspect, &bot, image));

        bot.config_hash = "b".repeat(64);
        assert!(!can_reuse(&inspect, &bot, image));

        bot.config_hash = "a".repeat(64);
        assert!(!can_reuse(&inspect, &bot, "sha256:new-image"));
    }

    #[test]
    fn unmanaged_or_stale_states_are_not_reused() {
        let bot = bot();
        let image = "sha256:image";
        let mut labels = managed_labels(&bot);
        labels.insert("anqa.managed".to_string(), "false".to_string());
        let unmanaged = inspect_with(ContainerStateStatusEnum::EXITED, false, labels, image);
        assert!(!can_reuse(&unmanaged, &bot, image));

        for state in [
            ContainerStateStatusEnum::DEAD,
            ContainerStateStatusEnum::PAUSED,
            ContainerStateStatusEnum::REMOVING,
            ContainerStateStatusEnum::EMPTY,
        ] {
            let inspect = inspect_with(state, false, managed_labels(&bot), image);
            assert!(!can_reuse(&inspect, &bot, image), "{state:?}");
        }
    }

    #[test]
    fn paused_and_running_states_are_handled_explicitly() {
        let bot = bot();
        let image = "sha256:image";
        let running = inspect_with(
            ContainerStateStatusEnum::RUNNING,
            false,
            managed_labels(&bot),
            image,
        );
        assert!(is_running_and_not_paused(&running));
        assert!(is_stoppable(&running));

        let paused = inspect_with(
            ContainerStateStatusEnum::PAUSED,
            true,
            managed_labels(&bot),
            image,
        );
        assert!(!is_running_and_not_paused(&paused));
        assert!(!is_stoppable(&paused));
        assert!(!has_reusable_state(&paused));

        let paused_running = inspect_with(
            ContainerStateStatusEnum::RUNNING,
            true,
            managed_labels(&bot),
            image,
        );
        assert!(!is_running_and_not_paused(&paused_running));
        assert!(!is_stoppable(&paused_running));
    }

    #[test]
    fn environment_is_sorted_and_joined() {
        assert_eq!(environment(&bot()), vec!["A=1", "B=2"]);
    }

    #[test]
    fn port_bindings_and_exposed_ports_are_generated() {
        let mut bot = bot();
        bot.ports = vec![
            PortSpec {
                host_ip: None,
                host_port: 8080,
                container_port: 80,
                protocol: PortProtocol::Tcp,
            },
            PortSpec {
                host_ip: Some("127.0.0.1".to_string()),
                host_port: 8443,
                container_port: 443,
                protocol: PortProtocol::Udp,
            },
        ];

        assert_eq!(exposed_ports(&bot), vec!["80/tcp", "443/udp"]);
        let bindings = port_bindings(&bot);
        assert_eq!(bindings.len(), 2);
        assert_eq!(
            bindings.get("80/tcp").unwrap().as_ref().unwrap()[0].host_port,
            Some("8080".to_string())
        );
        assert_eq!(
            bindings.get("443/udp").unwrap().as_ref().unwrap()[0].host_ip,
            Some("127.0.0.1".to_string())
        );
    }

    #[test]
    fn named_volumes_become_volume_mounts() {
        let mut bot = bot();
        bot.volumes = vec![VolumeSpec {
            name: "data".to_string(),
            target: PathBuf::from("/app/data"),
        }];

        let mounts = mounts(&bot);
        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].typ, Some(MountType::VOLUME));
        assert_eq!(mounts[0].source.as_deref(), Some("anqa-echo-bot-data"));
        assert_eq!(mounts[0].target.as_deref(), Some("/app/data"));

        let request = volume_request(&bot, &bot.volumes[0]);
        assert_eq!(request.name.as_deref(), Some("anqa-echo-bot-data"));
        let labels = request.labels.unwrap();
        assert_eq!(labels.get("anqa.managed").map(String::as_str), Some("true"));
        assert_eq!(labels.get("anqa.bot").map(String::as_str), Some("echo-bot"));
    }

    #[test]
    fn container_config_has_labels_logging_and_restart_policy() {
        let bot = bot();
        let config = container_config(&bot, "sha256:image", true);
        assert_eq!(config.image.as_deref(), Some("sha256:image"));
        assert_eq!(
            config.env.as_deref(),
            Some(&["A=1".to_string(), "B=2".to_string()][..])
        );
        assert_eq!(
            config.cmd.as_deref(),
            Some(&["run".to_string(), "--prod".to_string()][..])
        );

        let labels = config.labels.unwrap();
        assert_eq!(labels.get("anqa.managed").map(String::as_str), Some("true"));
        assert_eq!(labels.get("anqa.bot").map(String::as_str), Some("echo-bot"));
        assert_eq!(
            labels.get("anqa.config-hash").map(String::as_str),
            Some(bot.config_hash.as_str())
        );
        assert_eq!(
            labels.get("anqa.version").map(String::as_str),
            Some(env!("CARGO_PKG_VERSION"))
        );

        let host = config.host_config.unwrap();
        assert_eq!(host.auto_remove, Some(false));
        assert_eq!(
            host.restart_policy.unwrap().name,
            Some(RestartPolicyNameEnum::ALWAYS)
        );
        let log = host.log_config.unwrap();
        assert_eq!(log.typ.as_deref(), Some("json-file"));
        assert_eq!(
            log.config.unwrap(),
            HashMap::from([
                ("max-size".to_string(), LOG_MAX_SIZE.to_string()),
                ("max-file".to_string(), LOG_MAX_FILE.to_string()),
            ])
        );
    }

    #[test]
    fn name_conflicts_map_to_a_typed_lifecycle_error() {
        let error = map_create_error(
            "anqa-echo-bot",
            DockerError::Conflict {
                message: "name already in use".to_string(),
            },
        );
        assert!(matches!(
            error,
            LifecycleError::NameConflict { ref name } if name == "anqa-echo-bot"
        ));
        assert_eq!(
            error.to_string(),
            "container anqa-echo-bot is not managed by Anqa"
        );

        let error = map_create_error("anqa-echo-bot", DockerError::Unavailable);
        assert!(matches!(
            error,
            LifecycleError::Docker(DockerError::Unavailable)
        ));
    }

    #[test]
    fn autostart_is_read_from_the_restart_policy() {
        let bot = bot();
        let mut inspect = inspect_with(
            ContainerStateStatusEnum::EXITED,
            false,
            managed_labels(&bot),
            "sha256:image",
        );
        assert!(!restart_policy_enabled(&inspect));
        inspect.host_config = Some(HostConfig {
            restart_policy: Some(restart_policy(true)),
            ..HostConfig::default()
        });
        assert!(restart_policy_enabled(&inspect));
        inspect.host_config = Some(HostConfig {
            restart_policy: Some(restart_policy(false)),
            ..HostConfig::default()
        });
        assert!(!restart_policy_enabled(&inspect));
    }

    #[test]
    fn health_status_is_not_part_of_lifecycle_reuse() {
        let bot = bot();
        let mut inspect = inspect_with(
            ContainerStateStatusEnum::RUNNING,
            false,
            managed_labels(&bot),
            "sha256:image",
        );
        inspect.state.as_mut().unwrap().health = Some(bollard::models::Health {
            status: Some(HealthStatusEnum::UNHEALTHY),
            ..bollard::models::Health::default()
        });
        assert!(can_reuse(&inspect, &bot, "sha256:image"));
    }

    #[test]
    fn log_config_has_the_documented_rotation_limits() {
        let host = host_config(&bot(), false);
        let log: HostConfigLogConfig = host.log_config.unwrap();
        let config = log.config.unwrap();
        assert_eq!(config["max-size"], "10m");
        assert_eq!(config["max-file"], "3");
    }
}
