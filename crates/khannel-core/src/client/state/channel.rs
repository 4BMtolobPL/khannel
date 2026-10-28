use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct ChannelState {
    pub name: String,
    pub locked: bool,
}
