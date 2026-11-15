use bollard::models::{
    ContainerInspectResponse, ContainerStateStatusEnum, HealthStatusEnum, RestartPolicyNameEnum,
};
use serde::Serialize;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BotStatus {
    Running,
    Stopped,
    Starting,
    Restarting,
    Unhealthy,
    Error,
    Building,
    Stopping,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    Healthy,
    Unhealthy,
    Starting,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContainerView {
    pub status: BotStatus,
    pub health: Option<HealthStatus>,
    pub uptime_seconds: u64,
    pub autostart: bool,
}

pub fn from_inspect(inspect: &ContainerInspectResponse) -> ContainerView {
    let autostart = inspect
        .host_config
        .as_ref()
        .and_then(|config| config.restart_policy.as_ref())
        .and_then(|policy| policy.name)
        == Some(RestartPolicyNameEnum::ALWAYS);

    let Some(state) = inspect.state.as_ref() else {
        return ContainerView {
            status: BotStatus::Error,
            health: None,
            uptime_seconds: 0,
            autostart,
        };
    };

    let health = health_from(state.health.as_ref().and_then(|health| health.status));
    let status = match state.status {
        Some(ContainerStateStatusEnum::RUNNING) => match health {
            Some(HealthStatus::Starting) => BotStatus::Starting,
            Some(HealthStatus::Unhealthy) => BotStatus::Unhealthy,
            Some(HealthStatus::Healthy) | None => BotStatus::Running,
        },
        Some(ContainerStateStatusEnum::RESTARTING) => BotStatus::Restarting,
        Some(ContainerStateStatusEnum::STOPPING | ContainerStateStatusEnum::REMOVING) => {
            BotStatus::Stopping
        }
        Some(ContainerStateStatusEnum::DEAD) => BotStatus::Error,
        Some(
            ContainerStateStatusEnum::CREATED
            | ContainerStateStatusEnum::EXITED
            | ContainerStateStatusEnum::PAUSED
            | ContainerStateStatusEnum::EMPTY,
        )
        | None => BotStatus::Stopped,
    };

    let uptime_seconds = if state.status == Some(ContainerStateStatusEnum::RUNNING) {
        uptime_seconds(state.started_at.as_deref(), OffsetDateTime::now_utc())
    } else {
        0
    };

    ContainerView {
        status,
        health,
        uptime_seconds,
        autostart,
    }
}

fn health_from(status: Option<HealthStatusEnum>) -> Option<HealthStatus> {
    match status? {
        HealthStatusEnum::HEALTHY => Some(HealthStatus::Healthy),
        HealthStatusEnum::UNHEALTHY => Some(HealthStatus::Unhealthy),
        HealthStatusEnum::STARTING => Some(HealthStatus::Starting),
        HealthStatusEnum::NONE | HealthStatusEnum::EMPTY => None,
    }
}

pub fn uptime_seconds(started_at: Option<&str>, now: OffsetDateTime) -> u64 {
    let Some(started_at) = started_at else {
        return 0;
    };
    let started_at = match OffsetDateTime::parse(started_at, &Rfc3339) {
        Ok(started_at) => started_at,
        Err(error) => {
            tracing::debug!(%error, "could not parse container start time");
            return 0;
        }
    };
    u64::try_from((now - started_at).whole_seconds()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bollard::models::{
        ContainerInspectResponse, ContainerState, ContainerStateStatusEnum, Health,
        HealthStatusEnum, HostConfig, RestartPolicy, RestartPolicyNameEnum,
    };
    use time::{OffsetDateTime, format_description::well_known::Rfc3339};

    fn inspect(
        status: ContainerStateStatusEnum,
        health: Option<HealthStatusEnum>,
        restart: Option<RestartPolicyNameEnum>,
    ) -> ContainerInspectResponse {
        let state = ContainerState {
            status: Some(status),
            health: health.map(|health| Health {
                status: Some(health),
                ..Health::default()
            }),
            ..ContainerState::default()
        };
        let mut response = ContainerInspectResponse {
            state: Some(state),
            ..ContainerInspectResponse::default()
        };
        if let Some(restart) = restart {
            response.host_config = Some(HostConfig {
                restart_policy: Some(RestartPolicy {
                    name: Some(restart),
                    ..RestartPolicy::default()
                }),
                ..HostConfig::default()
            });
        }
        response
    }

    fn status_of(
        status: ContainerStateStatusEnum,
        health: Option<HealthStatusEnum>,
    ) -> ContainerView {
        from_inspect(&inspect(status, health, None))
    }

    #[test]
    fn maps_the_documented_states() {
        let cases = [
            (
                ContainerStateStatusEnum::RUNNING,
                HealthStatusEnum::NONE,
                BotStatus::Running,
            ),
            (
                ContainerStateStatusEnum::RUNNING,
                HealthStatusEnum::EMPTY,
                BotStatus::Running,
            ),
            (
                ContainerStateStatusEnum::RUNNING,
                HealthStatusEnum::HEALTHY,
                BotStatus::Running,
            ),
            (
                ContainerStateStatusEnum::RUNNING,
                HealthStatusEnum::STARTING,
                BotStatus::Starting,
            ),
            (
                ContainerStateStatusEnum::RUNNING,
                HealthStatusEnum::UNHEALTHY,
                BotStatus::Unhealthy,
            ),
            (
                ContainerStateStatusEnum::RESTARTING,
                HealthStatusEnum::NONE,
                BotStatus::Restarting,
            ),
            (
                ContainerStateStatusEnum::STOPPING,
                HealthStatusEnum::NONE,
                BotStatus::Stopping,
            ),
            (
                ContainerStateStatusEnum::REMOVING,
                HealthStatusEnum::NONE,
                BotStatus::Stopping,
            ),
            (
                ContainerStateStatusEnum::CREATED,
                HealthStatusEnum::NONE,
                BotStatus::Stopped,
            ),
            (
                ContainerStateStatusEnum::EXITED,
                HealthStatusEnum::NONE,
                BotStatus::Stopped,
            ),
            (
                ContainerStateStatusEnum::PAUSED,
                HealthStatusEnum::NONE,
                BotStatus::Stopped,
            ),
            (
                ContainerStateStatusEnum::EMPTY,
                HealthStatusEnum::NONE,
                BotStatus::Stopped,
            ),
            (
                ContainerStateStatusEnum::DEAD,
                HealthStatusEnum::NONE,
                BotStatus::Error,
            ),
        ];

        for (state, health, expected) in cases {
            assert_eq!(status_of(state, Some(health)).status, expected, "{state:?}");
        }
    }

    #[test]
    fn a_missing_state_is_an_error_card_status() {
        let view = from_inspect(&ContainerInspectResponse::default());
        assert_eq!(view.status, BotStatus::Error);
        assert_eq!(view.health, None);
        assert_eq!(view.uptime_seconds, 0);
    }

    #[test]
    fn health_mapping_preserves_none_and_the_three_named_states() {
        assert_eq!(
            status_of(
                ContainerStateStatusEnum::RUNNING,
                Some(HealthStatusEnum::NONE)
            )
            .health,
            None
        );
        assert_eq!(
            status_of(
                ContainerStateStatusEnum::RUNNING,
                Some(HealthStatusEnum::EMPTY)
            )
            .health,
            None
        );
        assert_eq!(
            status_of(
                ContainerStateStatusEnum::RUNNING,
                Some(HealthStatusEnum::STARTING)
            )
            .health,
            Some(HealthStatus::Starting)
        );
        assert_eq!(
            status_of(
                ContainerStateStatusEnum::RUNNING,
                Some(HealthStatusEnum::HEALTHY)
            )
            .health,
            Some(HealthStatus::Healthy)
        );
        assert_eq!(
            status_of(
                ContainerStateStatusEnum::RUNNING,
                Some(HealthStatusEnum::UNHEALTHY)
            )
            .health,
            Some(HealthStatus::Unhealthy)
        );
    }

    #[test]
    fn autostart_is_only_always() {
        for (policy, expected) in [
            (RestartPolicyNameEnum::ALWAYS, true),
            (RestartPolicyNameEnum::NO, false),
            (RestartPolicyNameEnum::UNLESS_STOPPED, false),
            (RestartPolicyNameEnum::ON_FAILURE, false),
            (RestartPolicyNameEnum::EMPTY, false),
        ] {
            let view = from_inspect(&inspect(
                ContainerStateStatusEnum::EXITED,
                None,
                Some(policy),
            ));
            assert_eq!(view.autostart, expected, "{policy:?}");
        }
        assert!(!from_inspect(&inspect(ContainerStateStatusEnum::EXITED, None, None)).autostart);
    }

    #[test]
    fn uptime_uses_fixed_now_and_parses_rfc3339() {
        let now = OffsetDateTime::parse("2026-01-01T00:10:00Z", &Rfc3339).unwrap();
        assert_eq!(uptime_seconds(None, now), 0);
        assert_eq!(uptime_seconds(Some("not-a-date"), now), 0);
        assert_eq!(uptime_seconds(Some("2026-01-01T00:00:00Z"), now), 600);
        assert_eq!(uptime_seconds(Some("2026-01-01T00:20:00Z"), now), 0);
    }

    #[test]
    fn not_running_containers_have_zero_uptime() {
        let mut response = inspect(ContainerStateStatusEnum::EXITED, None, None);
        response.state.as_mut().unwrap().started_at = Some("2026-01-01T00:09:00Z".into());
        assert_eq!(from_inspect(&response).uptime_seconds, 0);
    }

    #[test]
    fn statuses_and_health_serialize_lowercase() {
        let response = inspect(
            ContainerStateStatusEnum::RUNNING,
            Some(HealthStatusEnum::STARTING),
            Some(RestartPolicyNameEnum::ALWAYS),
        );
        let view = from_inspect(&response);
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["status"], "starting");
        assert_eq!(json["health"], "starting");
        assert_eq!(json["autostart"], true);

        assert_eq!(format!("{:?}", BotStatus::Unhealthy), "Unhealthy");
        assert_eq!(format!("{:?}", HealthStatus::Healthy), "Healthy");
    }
}
