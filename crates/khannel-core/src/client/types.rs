pub mod server;

use serde::Serialize;
use std::fmt::{Display, Formatter};
use uuid::Uuid;

pub type ServerId = Uuid;
pub type ChannelId = String;

/// 프론트 전달용 State
#[derive(Serialize, Clone, Debug, PartialEq)]
pub enum ServerStatus {
    Connecting,
    Connected,
    Registering,
    Disconnected,
    Disconnecting,
    Failed,
}

pub enum ServerCommand {
    Join(String),
    Privmsg { target: String, message: String },
    Part { channel_name: String },
    Nick(String),
    Quit,
}

impl Display for ServerCommand {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ServerCommand::Join(x) => write!(f, "Join, {x}"),
            ServerCommand::Privmsg { target, message } => write!(f, "Privmsg, {target}, {message}"),
            ServerCommand::Part { channel_name } => write!(f, "Part, {channel_name}"),
            ServerCommand::Nick(new_nick) => write!(f, "Nick, {new_nick}"),
            ServerCommand::Quit => write!(f, "Quit"),
        }
    }
}
