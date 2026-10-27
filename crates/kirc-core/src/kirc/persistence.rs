use crate::kirc::state::channel::ChannelState;
use crate::kirc::state::kirc::KircState;
use crate::kirc::state::server::ServerState;
use crate::kirc::types::server::ServerConfig;
use crate::kirc::types::ChannelId;
use crate::memento::Memento;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Serialize, Deserialize)]
pub(super) struct ServerStateSnapshot {
    config: ServerConfig,
    channels: HashMap<ChannelId, ChannelState>,
}

impl ServerStateSnapshot {
    pub(super) fn new(config: ServerConfig, channels: HashMap<ChannelId, ChannelState>) -> Self {
        Self { config, channels }
    }
}

impl Memento<ServerState> for ServerStateSnapshot {
    fn restore(self) -> ServerState {
        ServerState::with_channel(self.config, self.channels)
    }
}

#[derive(Serialize, Deserialize, Default)]
pub(crate) struct KircStateSnapshot {
    servers: Vec<ServerStateSnapshot>,
}

impl From<Vec<ServerStateSnapshot>> for KircStateSnapshot {
    fn from(value: Vec<ServerStateSnapshot>) -> Self {
        Self { servers: value }
    }
}

impl FromIterator<ServerStateSnapshot> for KircStateSnapshot {
    fn from_iter<T: IntoIterator<Item = ServerStateSnapshot>>(iter: T) -> Self {
        Self {
            servers: iter.into_iter().collect(),
        }
    }
}

impl Memento<KircState> for KircStateSnapshot {
    fn restore(self) -> KircState {
        KircState::from_iter(self.servers)
    }
}
