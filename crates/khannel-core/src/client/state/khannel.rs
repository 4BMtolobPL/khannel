use crate::client::persistence::{KhannelStateSnapshot, ServerStateSnapshot};
use crate::client::state::server::{ServerRuntime, ServerState};
use crate::client::types::ServerId;
use crate::client::types::server::ServerConfig;
use crate::memento::{Memento, Originator};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tracing::{debug, warn};
use uuid::Uuid;

pub struct KhannelState {
    servers: Mutex<HashMap<ServerId, Arc<ServerState>>>,
    persistence_path: Option<PathBuf>,
    /// 동시 저장 직렬화 — 같은 `.tmp` 파일에 두 저장이 섞여 쓰이지 않게 한다.
    save_lock: Mutex<()>,
    /// `drain_runtimes` 이후 true. 서버 맵이 비었으므로 이후 저장은 config.json을
    /// 빈 상태로 덮어쓰게 된다 → 저장을 막는다 (최종 저장은 drain 이전 `prepare_shutdown`).
    drained: AtomicBool,
}

impl KhannelState {
    pub fn set_persistence_path(&mut self, path: &std::path::Path) {
        self.persistence_path = Some(path.to_path_buf());
    }

    pub fn get_server(&self, server_id: ServerId) -> Option<Arc<ServerState>> {
        self.servers.lock().unwrap().get(&server_id).cloned()
    }

    pub fn get_all_servers(&self) -> HashMap<ServerId, Arc<ServerState>> {
        self.servers.lock().unwrap().clone()
    }

    pub fn add_server(&self, config: ServerConfig) -> anyhow::Result<ServerId> {
        let server_id = Uuid::now_v7();

        self.servers.lock().unwrap().insert(
            server_id,
            Arc::new(ServerState::new(ServerRuntime::Disconnected, config)),
        );

        self.save_snapshot()?;

        Ok(server_id)
    }

    /// 서버를 상태에서 빼낸다 (없으면 None). 저장은 호출자가 한다.
    pub fn remove_server(&self, server_id: ServerId) -> Option<Arc<ServerState>> {
        self.servers.lock().unwrap().remove(&server_id)
    }

    pub fn drain_runtimes(&self) -> Vec<ServerRuntime> {
        let mut guard = self.servers.lock().unwrap();
        self.drained.store(true, Ordering::Release);
        guard
            .drain()
            .map(|(_, state)| state.take_runtime())
            .collect()
    }

    pub fn is_channel_locked(&self, server_id: ServerId, channel: &str) -> bool {
        self.get_server(server_id)
            .map(|s| s.is_channel_locked(channel))
            .unwrap_or(false)
    }

    pub fn save_snapshot(&self) -> anyhow::Result<()> {
        let Some(path) = &self.persistence_path else {
            anyhow::bail!("No persistence path was provided")
        };

        let _save = self.save_lock.lock().unwrap();
        if self.drained.load(Ordering::Acquire) {
            debug!("Skip snapshot save after drain (would overwrite config with empty state)");
            return Ok(());
        }
        let snapshot = self.snapshot();
        crate::fs::save(path, snapshot)
    }

    /// 상태 변경(lock/join/part) 직후 영속화. 저장 실패는 명령 자체를 실패시키지 않고 로깅만 한다.
    pub fn persist(&self) {
        if let Err(e) = self.save_snapshot() {
            warn!(error = %e, "Failed to save snapshot");
        }
    }
}

impl FromIterator<ServerStateSnapshot> for KhannelState {
    fn from_iter<T: IntoIterator<Item = ServerStateSnapshot>>(iter: T) -> Self {
        let mut server_map = HashMap::new();
        for server in iter {
            server_map.insert(Uuid::now_v7(), Arc::new(server.restore()));
        }

        Self {
            servers: Mutex::new(server_map),
            persistence_path: None,
            save_lock: Mutex::new(()),
            drained: AtomicBool::new(false),
        }
    }
}

impl Originator<KhannelStateSnapshot> for KhannelState {
    fn snapshot(&self) -> KhannelStateSnapshot {
        let servers = self.servers.lock().unwrap();
        KhannelStateSnapshot::from_iter(servers.values().map(|state| state.snapshot()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::types::server::ServerConfig;

    /// drain 이후의 저장은 config.json을 빈 서버 목록으로 덮어쓰지 않는다.
    #[test]
    fn save_after_drain_keeps_last_snapshot() {
        let path = std::env::temp_dir().join(format!("khannel-test-{}.json", Uuid::now_v7()));
        let config = ServerConfig::new("irc.test".to_string(), 6667, false, "nick".to_string());
        let mut state = KhannelState::from_iter([ServerStateSnapshot::new(config, HashMap::new())]);
        state.set_persistence_path(&path);

        state.save_snapshot().unwrap();
        let before = std::fs::read_to_string(&path).unwrap();

        state.drain_runtimes();
        state.save_snapshot().unwrap();
        state.persist();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        assert!(before.contains("irc.test"));

        let _ = std::fs::remove_file(&path);
    }
}
