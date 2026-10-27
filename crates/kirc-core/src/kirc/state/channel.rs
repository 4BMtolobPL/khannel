use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub(in crate::kirc) struct ChannelState {
    pub(in crate::kirc) name: String,
    pub(in crate::kirc) locked: bool,
}
