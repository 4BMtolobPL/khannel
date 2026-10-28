use crate::client::state::channel::ChannelState;
use crate::client::state::khannel::KhannelState;
use crate::client::state::server::ServerState;
use crate::client::types::ChannelId;
use crate::client::types::server::ServerConfig;
use crate::memento::Memento;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Serialize, Deserialize)]
pub struct ServerStateSnapshot {
    config: ServerConfig,
    channels: HashMap<ChannelId, ChannelState>,
}

impl ServerStateSnapshot {
    pub fn new(config: ServerConfig, channels: HashMap<ChannelId, ChannelState>) -> Self {
        Self { config, channels }
    }
}

impl Memento<ServerState> for ServerStateSnapshot {
    fn restore(self) -> ServerState {
        ServerState::with_channel(self.config, self.channels)
    }
}

#[derive(Serialize, Deserialize, Default)]
pub struct KhannelStateSnapshot {
    servers: Vec<ServerStateSnapshot>,
}

impl From<Vec<ServerStateSnapshot>> for KhannelStateSnapshot {
    fn from(value: Vec<ServerStateSnapshot>) -> Self {
        Self { servers: value }
    }
}

impl FromIterator<ServerStateSnapshot> for KhannelStateSnapshot {
    fn from_iter<T: IntoIterator<Item = ServerStateSnapshot>>(iter: T) -> Self {
        Self {
            servers: iter.into_iter().collect(),
        }
    }
}

impl Memento<KhannelState> for KhannelStateSnapshot {
    fn restore(self) -> KhannelState {
        KhannelState::from_iter(self.servers)
    }
}
