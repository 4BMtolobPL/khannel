use crate::kirc::persistence::ServerStateSnapshot;
use crate::kirc::state::channel::ChannelState;
use crate::kirc::types::server::ServerConfig;
use crate::kirc::types::{ChannelId, ServerCommand, ServerStatus};
use crate::memento::Originator;
use anyhow::anyhow;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tracing::debug;

#[derive(Default)]
pub(in crate::kirc) enum ServerRuntime {
    #[default]
    Disconnected,
    Connecting {
        handle: JoinHandle<()>,
    },
    Registering {
        tx: UnboundedSender<ServerCommand>,
        handle: JoinHandle<()>,
    },
    Connected {
        tx: UnboundedSender<ServerCommand>,
        handle: JoinHandle<()>,
    },
    Disconnecting {
        handle: JoinHandle<()>,
    },
    Failed {
        error: String,
    },
}

impl ServerRuntime {
    fn status(&self) -> ServerStatus {
        match self {
            ServerRuntime::Disconnected => ServerStatus::Disconnected,
            ServerRuntime::Connecting { .. } => ServerStatus::Connecting,
            ServerRuntime::Registering { .. } => ServerStatus::Registering,
            ServerRuntime::Connected { .. } => ServerStatus::Connected,
            ServerRuntime::Disconnecting { .. } => ServerStatus::Disconnecting,
            ServerRuntime::Failed { .. } => ServerStatus::Failed,
        }
    }

    pub(in crate::kirc) async fn graceful_shutdown(self) {
        const TIMEOUT: Duration = Duration::from_secs(5);

        match self {
            ServerRuntime::Connected { tx, handle } | ServerRuntime::Registering { tx, handle } => {
                // 1. QUIT 전송
                debug!(command = %ServerCommand::Quit, "tx send");
                let _ = tx.send(ServerCommand::Quit);

                // 2. 정상 종료 대기 (timeout optional)
                let _ = timeout(TIMEOUT, handle).await;
            }

            ServerRuntime::Connecting { handle } => {
                handle.abort();
                let _ = handle.await;
            }

            ServerRuntime::Disconnecting { handle } => {
                let _ = timeout(TIMEOUT, handle).await;
            }

            ServerRuntime::Disconnected | ServerRuntime::Failed { .. } => {
                // nothing
            }
        }
    }
}

pub(in crate::kirc) struct ServerState {
    runtime: Mutex<ServerRuntime>,
    config: Mutex<ServerConfig>,
    channels: Mutex<HashMap<ChannelId, ChannelState>>,
    current_nickname: Mutex<String>,
}

impl ServerState {
    pub(in crate::kirc) fn new(runtime: ServerRuntime, config: ServerConfig) -> Self {
        Self {
            runtime: Mutex::new(runtime),
            current_nickname: Mutex::new(config.nickname().to_string()),
            config: Mutex::new(config),
            channels: Mutex::new(HashMap::new()),
        }
    }

    pub(in crate::kirc) fn with_channel(
        config: ServerConfig,
        channels: HashMap<ChannelId, ChannelState>,
    ) -> Self {
        Self {
            runtime: Mutex::new(ServerRuntime::Disconnected),
            current_nickname: Mutex::new(config.nickname().to_string()),
            config: Mutex::new(config),
            channels: Mutex::new(channels),
        }
    }

    pub(in crate::kirc) fn status(&self) -> ServerStatus {
        self.runtime.lock().unwrap().status()
    }

    pub(in crate::kirc) fn config(&self) -> ServerConfig {
        self.config.lock().unwrap().clone()
    }

    pub(in crate::kirc) fn channels(&self) -> HashMap<ChannelId, ChannelState> {
        self.channels.lock().unwrap().clone()
    }

    pub(in crate::kirc) fn insert_channel(&self, channel_name: &str, locked: bool) {
        self.channels.lock().unwrap().insert(
            channel_name.to_string(),
            ChannelState {
                name: channel_name.to_string(),
                locked,
            },
        );
    }

    pub(in crate::kirc) fn remove_channel(&self, channel_name: &str) -> Option<ChannelState> {
        self.channels.lock().unwrap().remove(channel_name)
    }

    pub(in crate::kirc) fn current_nickname(&self) -> String {
        self.current_nickname.lock().unwrap().clone()
    }

    pub(in crate::kirc) fn set_current_nickname(&self, new_nick: &str) {
        *self.current_nickname.lock().unwrap() = new_nick.to_string();
    }

    pub(in crate::kirc) fn is_active(&self) -> bool {
        matches!(
            &*self.runtime.lock().unwrap(),
            ServerRuntime::Connecting { .. }
                | ServerRuntime::Registering { .. }
                | ServerRuntime::Connected { .. }
        )
    }

    pub(in crate::kirc) fn is_channel_locked(&self, channel: &str) -> bool {
        self.channels
            .lock()
            .unwrap()
            .get(channel)
            .map(|s| s.locked)
            .unwrap_or(false)
    }

    pub(in crate::kirc) fn set_channel_locked(&self, channel: &str, locked: bool) {
        if let Some(channel) = self.channels.lock().unwrap().get_mut(channel) {
            channel.locked = locked;
        }
    }

    pub(in crate::kirc) fn send_command(&self, cmd: ServerCommand) -> anyhow::Result<()> {
        let guard = self.runtime.lock().unwrap();
        match &*guard {
            ServerRuntime::Connected { tx, .. } | ServerRuntime::Registering { tx, .. } => {
                debug!(command = %cmd, "tx send");
                tx.send(cmd).map_err(|e| anyhow!("Failed to send: {}", e))
            }
            _ => Err(anyhow!("Server not connected")),
        }
    }

    pub(in crate::kirc) fn transition_to_connecting(&self, handle: JoinHandle<()>) {
        let mut guard = self.runtime.lock().unwrap();
        if let ServerRuntime::Disconnected | ServerRuntime::Failed { .. } =
            std::mem::take(&mut *guard)
        {
            *guard = ServerRuntime::Connecting { handle };
        }
    }

    pub(in crate::kirc) fn transition_to_registering(&self, tx: UnboundedSender<ServerCommand>) {
        let mut guard = self.runtime.lock().unwrap();
        if let ServerRuntime::Connecting { handle } = std::mem::take(&mut *guard) {
            *guard = ServerRuntime::Registering { tx, handle };
        }
    }

    pub(in crate::kirc) fn transition_to_connected(&self) {
        let mut guard = self.runtime.lock().unwrap();
        if let ServerRuntime::Registering { tx, handle } = std::mem::take(&mut *guard) {
            *guard = ServerRuntime::Connected { tx, handle };
        }
    }

    pub(in crate::kirc) fn transition_to_disconnected(&self) {
        *self.runtime.lock().unwrap() = ServerRuntime::Disconnected;
    }

    pub(in crate::kirc) fn transition_to_failed(&self, error: String) {
        *self.runtime.lock().unwrap() = ServerRuntime::Failed { error };
    }

    pub(in crate::kirc) fn disconnect(&self) {
        let mut guard = self.runtime.lock().unwrap();
        match std::mem::take(&mut *guard) {
            ServerRuntime::Registering { tx, handle } | ServerRuntime::Connected { tx, handle } => {
                debug!(command = %ServerCommand::Quit, "tx send");
                let _ = tx.send(ServerCommand::Quit);
                *guard = ServerRuntime::Disconnecting { handle };
            }
            other => {
                *guard = other;
            }
        }
    }

    pub(in crate::kirc) fn abort_connecting(&self) -> bool {
        let mut guard = self.runtime.lock().unwrap();
        if let ServerRuntime::Connecting { handle } = std::mem::take(&mut *guard) {
            handle.abort();
            *guard = ServerRuntime::Disconnected;
            true
        } else {
            false
        }
    }

    pub(in crate::kirc) fn take_runtime(&self) -> ServerRuntime {
        std::mem::take(&mut *self.runtime.lock().unwrap())
    }
}

impl Originator<ServerStateSnapshot> for ServerState {
    fn snapshot(&self) -> ServerStateSnapshot {
        ServerStateSnapshot::new(self.config(), self.channels())
    }
}
