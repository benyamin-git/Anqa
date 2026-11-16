use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::auth::Auth;
use crate::config::Config;
use crate::docker::DockerClient;

const MAX_OPERATION_ERROR: usize = 2048;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub docker: DockerClient,
    pub auth: Arc<Auth>,
    pub ops: Arc<BotOps>,
    pub version: String,
}

impl AppState {
    pub fn new(config: Config, docker: DockerClient, auth: Auth) -> Self {
        Self {
            config: Arc::new(config),
            docker,
            auth: Arc::new(auth),
            ops: Arc::new(BotOps::default()),
            version: crate::version().to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BotPhase {
    Building,
    Starting,
    Stopping,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct BotOpSnapshot {
    pub busy: bool,
    pub phase: Option<BotPhase>,
    pub error: Option<String>,
}

#[derive(Clone, Default)]
pub struct BotOps {
    inner: Arc<Mutex<HashMap<String, BotOp>>>,
}

impl BotOps {
    pub fn begin(&self, dir: &str, phase: Option<BotPhase>) -> Option<OperationGuard> {
        let mut map = lock(&self.inner);
        let op = map.entry(dir.to_string()).or_default();
        if op.busy {
            return None;
        }

        let lock = op.lock.clone();
        let guard = lock.try_lock_owned().ok()?;
        op.busy = true;
        op.phase = phase;

        Some(OperationGuard {
            ops: Arc::clone(&self.inner),
            dir: dir.to_string(),
            _lock: guard,
        })
    }

    pub fn snapshot(&self, dir: &str) -> BotOpSnapshot {
        lock(&self.inner)
            .get(dir)
            .map(|op| BotOpSnapshot {
                busy: op.busy,
                phase: op.phase,
                error: op.error.clone(),
            })
            .unwrap_or_default()
    }
}

pub struct OperationGuard {
    ops: Arc<Mutex<HashMap<String, BotOp>>>,
    dir: String,
    _lock: tokio::sync::OwnedMutexGuard<()>,
}

impl OperationGuard {
    pub fn finish(&mut self, error: Option<String>) {
        if let Some(op) = lock(&self.ops).get_mut(&self.dir) {
            op.error = error.map(truncate_error);
        }
    }
}

impl Drop for OperationGuard {
    fn drop(&mut self) {
        if let Some(op) = lock(&self.ops).get_mut(&self.dir) {
            op.busy = false;
            op.phase = None;
        }
    }
}

#[derive(Default)]
struct BotOp {
    lock: Arc<tokio::sync::Mutex<()>>,
    busy: bool,
    phase: Option<BotPhase>,
    error: Option<String>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn truncate_error(mut error: String) -> String {
    if error.len() <= MAX_OPERATION_ERROR {
        return error;
    }
    let mut end = MAX_OPERATION_ERROR;
    while !error.is_char_boundary(end) {
        end -= 1;
    }
    error.truncate(end);
    error
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn begin_is_exclusive_and_drop_releases_the_bot() {
        let ops = BotOps::default();
        let guard = ops.begin("echo", Some(BotPhase::Building)).unwrap();
        assert!(ops.snapshot("echo").busy);
        assert_eq!(ops.snapshot("echo").phase, Some(BotPhase::Building));
        assert!(ops.begin("echo", None).is_none());
        assert!(ops.begin("other", None).is_some());

        drop(guard);
        let snapshot = ops.snapshot("echo");
        assert!(!snapshot.busy);
        assert_eq!(snapshot.phase, None);
    }

    #[test]
    fn finish_records_and_clears_errors_without_releasing_busy() {
        let ops = BotOps::default();
        let mut guard = ops.begin("echo", Some(BotPhase::Starting)).unwrap();
        guard.finish(Some("build failed".to_string()));
        let snapshot = ops.snapshot("echo");
        assert!(snapshot.busy);
        assert_eq!(snapshot.error.as_deref(), Some("build failed"));

        guard.finish(None);
        assert_eq!(ops.snapshot("echo").error, None);
    }

    #[test]
    fn operation_errors_are_truncated_on_a_utf8_boundary() {
        let long = "a".repeat(MAX_OPERATION_ERROR + 10);
        assert_eq!(truncate_error(long).len(), MAX_OPERATION_ERROR);

        let mut text = "a".repeat(MAX_OPERATION_ERROR - 1);
        text.push('é');
        text.push_str(&"b".repeat(100));
        let truncated = truncate_error(text);
        assert!(truncated.len() <= MAX_OPERATION_ERROR);
        assert!(truncated.is_char_boundary(truncated.len()));
    }
}
