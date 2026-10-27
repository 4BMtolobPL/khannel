use crate::kirc::persistence::{KircStateSnapshot, ServerStateSnapshot};
use crate::kirc::state::server::{ServerRuntime, ServerState};
use crate::kirc::types::server::ServerConfig;
use crate::kirc::types::ServerId;
use crate::memento::{Memento, Originator};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

pub(crate) struct KircState {
    servers: Mutex<HashMap<ServerId, Arc<ServerState>>>,
    persistence_path: Option<PathBuf>,
}

impl KircState {
    pub(crate) fn set_persistence_path(&mut self, path: &std::path::Path) {
        self.persistence_path = Some(path.to_path_buf());
    }

    pub(in crate::kirc) fn get_server(&self, server_id: ServerId) -> Option<Arc<ServerState>> {
        self.servers.lock().unwrap().get(&server_id).cloned()
    }

    pub(in crate::kirc) fn get_all_servers(&self) -> HashMap<ServerId, Arc<ServerState>> {
        self.servers.lock().unwrap().clone()
    }

    pub(in crate::kirc) fn add_server(&self, config: ServerConfig) -> anyhow::Result<ServerId> {
        let server_id = Uuid::now_v7();

        self.servers.lock().unwrap().insert(
            server_id,
            Arc::new(ServerState::new(ServerRuntime::Disconnected, config)),
        );

        self.save_snapshot()?;

        Ok(server_id)
    }

    pub(in crate::kirc) fn drain_runtimes(&self) -> Vec<ServerRuntime> {
        let mut guard = self.servers.lock().unwrap();
        guard
            .drain()
            .map(|(_, state)| state.take_runtime())
            .collect()
    }

    pub(in crate::kirc) fn is_channel_locked(&self, server_id: ServerId, channel: &str) -> bool {
        self.get_server(server_id)
            .map(|s| s.is_channel_locked(channel))
            .unwrap_or(false)
    }

    pub(in crate::kirc) fn save_snapshot(&self) -> anyhow::Result<()> {
        if let Some(path) = &self.persistence_path {
            let snapshot = self.snapshot();
            crate::fs::save(path, snapshot)
        } else {
            anyhow::bail!("No persistence path was provided")
        }
    }
}

impl FromIterator<ServerStateSnapshot> for KircState {
    fn from_iter<T: IntoIterator<Item = ServerStateSnapshot>>(iter: T) -> Self {
        let mut server_map = HashMap::new();
        for server in iter {
            server_map.insert(Uuid::now_v7(), Arc::new(server.restore()));
        }

        Self {
            servers: Mutex::new(server_map),
            persistence_path: None,
        }
    }
}

impl Originator<KircStateSnapshot> for KircState {
    fn snapshot(&self) -> KircStateSnapshot {
        let servers = self.servers.lock().unwrap();
        KircStateSnapshot::from_iter(servers.values().map(|state| state.snapshot()))
    }
}
