use crate::kirc::emits::payload::{
    ChangeNickFailedPayload, ChannelLockChangedEvent, ServerDetail, ServerStatusPayload,
    SystemMessagePayload, UIEventPayload,
};
use crate::kirc::types::{ServerId, ServerStatus};
use tauri::{AppHandle, Emitter};
use tracing::trace;

pub(super) fn emit_server_added(
    app_handle: &AppHandle,
    server_id: ServerId,
    host: &str,
    port: u16,
    tls: bool,
    nickname: &str,
    status: ServerStatus,
) -> anyhow::Result<()> {
    trace!("Emit emit_server_added");

    app_handle.emit(
        "kirc:server_added",
        ServerDetail::new(
            server_id,
            host.to_string(),
            port,
            tls,
            nickname.to_string(),
            status,
        ),
    )?;

    Ok(())
}

pub(super) fn emit_server_status(
    app_handle: &AppHandle,
    server_id: ServerId,
    status: ServerStatus,
) -> anyhow::Result<()> {
    trace!("Emit emit_server_status");

    app_handle.emit(
        "kirc:server_status",
        ServerStatusPayload::new(server_id, status),
    )?;

    Ok(())
}

pub(super) fn emit_channel_lock_changed(
    app_handle: &AppHandle,
    server_id: ServerId,
    channel: &str,
    locked: bool,
) -> anyhow::Result<()> {
    trace!("Emit emit_channel_lock_changed");

    app_handle.emit(
        "kirc:channel_lock_changed",
        ChannelLockChangedEvent::new(server_id, channel.to_string(), locked),
    )?;

    Ok(())
}

pub(super) fn emit_ui_event(app_handle: &AppHandle) -> UIEventBuilder {
    UIEventBuilder::new(app_handle.clone())
}

pub(super) struct UIEventBuilder {
    app_handle: AppHandle,
    payload: Option<UIEventPayload>,
}

impl UIEventBuilder {
    fn new(app_handle: AppHandle) -> Self {
        Self {
            app_handle,
            payload: None,
        }
    }

    pub(super) fn emit(self) -> anyhow::Result<()> {
        if let Some(payload) = self.payload {
            trace!("Emit emit_ui_event: {:?}", payload);

            self.app_handle.emit("kirc:event", payload)?;
            Ok(())
        } else {
            anyhow::bail!("Event payload is missing");
        }
    }

    pub(super) fn user_message(
        mut self,
        server_id: ServerId,
        channel: String,
        nickname: String,
        content: String,
    ) -> Self {
        self.payload = Some(UIEventPayload::UserMessage {
            server_id,
            channel,
            nick: nickname,
            content,
            timestamp: chrono::Utc::now().timestamp_millis() as u64,
        });

        self
    }

    pub(super) fn join(mut self, server_id: ServerId, channel: String, nickname: String) -> Self {
        self.payload = Some(UIEventPayload::Join {
            server_id,
            channel,
            nick: nickname,
        });

        self
    }

    pub(super) fn part(
        mut self,
        server_id: ServerId,
        channels: String,
        nickname: String,
        reason: Option<String>,
    ) -> Self {
        self.payload = Some(UIEventPayload::Part {
            server_id,
            channel: channels,
            nick: nickname,
            reason,
        });

        self
    }

    pub(super) fn quit(
        mut self,
        server_id: ServerId,
        nickname: String,
        reason: Option<String>,
    ) -> Self {
        self.payload = Some(UIEventPayload::Quit {
            server_id,
            nick: nickname,
            reason,
        });

        self
    }

    pub(super) fn nick(mut self, server_id: ServerId, old_nick: String, new_nick: String) -> Self {
        self.payload = Some(UIEventPayload::Nick {
            server_id,
            old_nick,
            new_nick,
        });

        self
    }

    pub(super) fn topic(
        mut self,
        server_id: ServerId,
        channel: String,
        topic: Option<String>,
    ) -> Self {
        self.payload = Some(UIEventPayload::Topic {
            server_id,
            channel,
            topic,
        });

        self
    }

    pub(super) fn error(mut self, server_id: ServerId, message: String) -> Self {
        self.payload = Some(UIEventPayload::Error { server_id, message });

        self
    }
}

pub(super) fn emit_system_message(
    app_handle: &AppHandle,
    server_id: ServerId,
    message: &str,
) -> anyhow::Result<()> {
    trace!("Emit system_message");

    app_handle.emit(
        "kirc:system_message",
        SystemMessagePayload::new(server_id, message),
    )?;
    Ok(())
}

pub(super) fn emit_change_nick_failed(
    app_handle: &AppHandle,
    server_id: ServerId,
    reason: &str,
) -> anyhow::Result<()> {
    app_handle.emit(
        "kirc:change_nick_failed",
        ChangeNickFailedPayload::new(server_id, reason),
    )?;

    Ok(())
}

mod payload {
    use crate::kirc::types::{ChannelId, ServerId, ServerStatus};
    use serde::Serialize;

    #[derive(Serialize, Clone)]
    #[serde(rename_all = "camelCase")]
    pub(super) struct ServerDetail {
        server_id: ServerId,
        host: String,
        port: u16,
        tls: bool,
        nickname: String,
        status: ServerStatus,
    }

    impl ServerDetail {
        pub(super) fn new(
            server_id: ServerId,
            host: String,
            port: u16,
            tls: bool,
            nickname: String,
            status: ServerStatus,
        ) -> Self {
            Self {
                server_id,
                host,
                port,
                tls,
                nickname,
                status,
            }
        }
    }

    #[derive(Serialize, Clone)]
    #[serde(rename_all = "camelCase")]
    pub(super) struct ServerStatusPayload {
        server_id: ServerId,
        status: ServerStatus,
    }

    impl ServerStatusPayload {
        pub(super) fn new(server_id: ServerId, status: ServerStatus) -> Self {
            Self { server_id, status }
        }
    }

    #[derive(Serialize, Clone)]
    #[serde(rename_all = "camelCase")]
    pub(super) struct ChannelLockChangedEvent {
        server_id: ServerId,
        channel: String,
        locked: bool,
    }

    impl ChannelLockChangedEvent {
        pub(super) fn new(server_id: ServerId, channel: ChannelId, locked: bool) -> Self {
            Self {
                server_id,
                channel,
                locked,
            }
        }
    }

    #[derive(Serialize, Clone)]
    #[serde(rename_all = "camelCase")]
    pub(super) struct SystemMessagePayload {
        server_id: ServerId,
        message: String,
    }

    impl SystemMessagePayload {
        pub(super) fn new(server_id: ServerId, message: &str) -> Self {
            Self {
                server_id,
                message: message.to_string(),
            }
        }
    }

    #[derive(Debug, Serialize, Clone)]
    #[serde(tag = "type")]
    pub(super) enum UIEventPayload {
        UserMessage {
            server_id: ServerId,
            channel: ChannelId,
            nick: String,
            content: String,
            timestamp: u64,
        },
        Join {
            server_id: ServerId,
            channel: ChannelId,
            nick: String,
        },
        Part {
            server_id: ServerId,
            channel: ChannelId,
            nick: String,
            reason: Option<String>,
        },
        Quit {
            server_id: ServerId,
            nick: String,
            reason: Option<String>,
        },
        Nick {
            server_id: ServerId,
            old_nick: String,
            new_nick: String,
        },
        Topic {
            server_id: ServerId,
            channel: ChannelId,
            topic: Option<String>,
        },
        Error {
            server_id: ServerId,
            message: String,
        },
    }

    #[derive(Serialize, Clone)]
    #[serde(rename_all = "camelCase")]
    pub(super) struct ChangeNickFailedPayload {
        server_id: ServerId,
        reason: String,
    }

    impl ChangeNickFailedPayload {
        pub(super) fn new(server_id: ServerId, reason: &str) -> Self {
            Self {
                server_id,
                reason: reason.to_string(),
            }
        }
    }
}
