//! UI 이벤트 버스.
//!
//! UI 프레임워크(GPUI 등)가 모두 사용하는 이벤트를 하나의 [`UiEvent`]로 정의하고,
//! 전달 경로는 [`UiSink`] trait으로 추상화한다. tokio 채널 구현체를 기본 제공한다.

use crate::client::types::{ChannelId, ServerId, ServerStatus};
use serde::Serialize;
use tracing::trace;

/// UI로 전달되는 모든 이벤트. emits.rs의 emit_* 7종을 하나로 통합했다.
#[derive(Debug, Clone)]
pub enum UiEvent {
    ServerAdded(ServerDetail),
    /// 서버 삭제 — UI는 서버와 그 채널/메시지를 지운다
    ServerRemoved(ServerRemovedPayload),
    ServerStatus(ServerStatusPayload),
    ChannelLockChanged(ChannelLockChangedEvent),
    SystemMessage(SystemMessagePayload),
    ChangeNickFailed(ChangeNickFailedPayload),
    Ui(UIEventPayload),
}

/// 이벤트 소비자가 어디(GPUI, 테스트, ...)에 붙든 동일하게 동작하도록 추상화.
pub trait UiSink: Send + Sync + 'static {
    fn emit(&self, event: UiEvent);
}

impl<T: UiSink + ?Sized> UiSink for std::sync::Arc<T> {
    fn emit(&self, event: UiEvent) {
        (**self).emit(event)
    }
}

/// UiSink를 구현하는 객체. 구현체는 채널 큐잉 실패가 절대 발생하면 안 되도록 send 실패를 로깅.
pub type EventSender = std::sync::Arc<dyn UiSink>;

impl UiSink for tokio::sync::mpsc::UnboundedSender<UiEvent> {
    fn emit(&self, event: UiEvent) {
        if let Err(e) = self.send(event) {
            tracing::error!(error = %e, "UiEvent queue send failed");
        }
    }
}

// ==================================== Helpers ====================================

pub fn emit_server_added(
    ui_sink: &EventSender,
    server_id: ServerId,
    host: &str,
    port: u16,
    tls: bool,
    nickname: &str,
    status: ServerStatus,
) {
    trace!("Emit UiEvent::ServerAdded");
    ui_sink.emit(UiEvent::ServerAdded(ServerDetail {
        server_id,
        host: host.to_string(),
        port,
        tls,
        nickname: nickname.to_string(),
        status,
    }));
}

pub fn emit_server_removed(ui_sink: &EventSender, server_id: ServerId) {
    trace!("Emit UiEvent::ServerRemoved");
    ui_sink.emit(UiEvent::ServerRemoved(ServerRemovedPayload { server_id }));
}

pub fn emit_server_status(ui_sink: &EventSender, server_id: ServerId, status: ServerStatus) {
    trace!("Emit UiEvent::ServerStatus");
    ui_sink.emit(UiEvent::ServerStatus(ServerStatusPayload {
        server_id,
        status,
    }));
}

pub fn emit_channel_lock_changed(
    ui_sink: &EventSender,
    server_id: ServerId,
    channel: &str,
    locked: bool,
) {
    trace!("Emit UiEvent::ChannelLockChanged");
    ui_sink.emit(UiEvent::ChannelLockChanged(ChannelLockChangedEvent {
        server_id,
        channel: channel.to_string(),
        locked,
    }));
}

pub fn emit_ui_event(ui_sink: &EventSender) -> UIEventBuilder {
    UIEventBuilder::new(ui_sink.clone())
}

pub struct UIEventBuilder {
    ui_sink: EventSender,
    payload: Option<UIEventPayload>,
}

impl UIEventBuilder {
    fn new(ui_sink: EventSender) -> Self {
        Self {
            ui_sink,
            payload: None,
        }
    }

    pub fn emit(self) {
        if let Some(payload) = self.payload {
            trace!("Emit UiEvent::Ui: {:?}", payload);
            self.ui_sink.emit(UiEvent::Ui(payload));
        } else {
            tracing::error!("UIEvent payload is missing");
        }
    }

    pub fn user_message(
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
            action: false,
            timestamp: chrono::Utc::now().timestamp_millis() as u64,
        });

        self
    }

    /// `/me` 액션 메시지 (CTCP ACTION). `content`는 `\x01ACTION`을 벗긴 본문
    pub fn action_message(
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
            action: true,
            timestamp: chrono::Utc::now().timestamp_millis() as u64,
        });

        self
    }

    pub fn notice(
        mut self,
        server_id: ServerId,
        target: String,
        nickname: Option<String>,
        content: String,
    ) -> Self {
        self.payload = Some(UIEventPayload::Notice {
            server_id,
            target,
            nick: nickname,
            content,
            timestamp: chrono::Utc::now().timestamp_millis() as u64,
        });

        self
    }

    pub fn join(mut self, server_id: ServerId, channel: String, nickname: String) -> Self {
        self.payload = Some(UIEventPayload::Join {
            server_id,
            channel,
            nick: nickname,
        });

        self
    }

    pub fn part(
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

    pub fn kick(
        mut self,
        server_id: ServerId,
        channel: String,
        nickname: String,
        by: String,
        reason: Option<String>,
    ) -> Self {
        self.payload = Some(UIEventPayload::Kick {
            server_id,
            channel,
            nick: nickname,
            by,
            reason,
        });

        self
    }

    pub fn quit(mut self, server_id: ServerId, nickname: String, reason: Option<String>) -> Self {
        self.payload = Some(UIEventPayload::Quit {
            server_id,
            nick: nickname,
            reason,
        });

        self
    }

    pub fn nick(mut self, server_id: ServerId, old_nick: String, new_nick: String) -> Self {
        self.payload = Some(UIEventPayload::Nick {
            server_id,
            old_nick,
            new_nick,
        });

        self
    }

    pub fn topic(
        mut self,
        server_id: ServerId,
        channel: String,
        topic: Option<String>,
        changed_by: Option<String>,
    ) -> Self {
        self.payload = Some(UIEventPayload::Topic {
            server_id,
            channel,
            topic,
            changed_by,
        });

        self
    }

    pub fn names(mut self, server_id: ServerId, channel: String, users: Vec<String>) -> Self {
        self.payload = Some(UIEventPayload::Names {
            server_id,
            channel,
            users,
        });

        self
    }

    pub fn error(mut self, server_id: ServerId, message: String) -> Self {
        self.payload = Some(UIEventPayload::Error { server_id, message });

        self
    }
}

pub fn emit_system_message(ui_sink: &EventSender, server_id: ServerId, message: &str) {
    trace!("Emit UiEvent::SystemMessage");

    ui_sink.emit(UiEvent::SystemMessage(SystemMessagePayload {
        server_id,
        message: message.to_string(),
    }));
}

pub fn emit_change_nick_failed(ui_sink: &EventSender, server_id: ServerId, reason: &str) {
    trace!("Emit UiEvent::ChangeNickFailed");

    ui_sink.emit(UiEvent::ChangeNickFailed(ChangeNickFailedPayload {
        server_id,
        reason: reason.to_string(),
    }));
}

// ==================================== Payloads ====================================

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerDetail {
    pub server_id: ServerId,
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub nickname: String,
    pub status: ServerStatus,
}

#[derive(Debug, Clone)]
pub struct ServerRemovedPayload {
    pub server_id: ServerId,
}

#[derive(Debug, Clone)]
pub struct ServerStatusPayload {
    pub server_id: ServerId,
    pub status: ServerStatus,
}

#[derive(Debug, Clone)]
pub struct ChannelLockChangedEvent {
    pub server_id: ServerId,
    pub channel: ChannelId,
    pub locked: bool,
}

#[derive(Debug, Clone)]
pub struct SystemMessagePayload {
    pub server_id: ServerId,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum UIEventPayload {
    UserMessage {
        server_id: ServerId,
        channel: ChannelId,
        nick: String,
        content: String,
        /// `/me` 액션(CTCP ACTION)이면 true — UI는 `* nick 내용`으로 표시한다
        action: bool,
        timestamp: u64,
    },
    /// NOTICE. `target`은 받은 그대로(채널/내 닉/`*` 등) — 어느 버퍼에 쌓을지는 UI가 정한다
    Notice {
        server_id: ServerId,
        target: String,
        /// 보낸 닉. 서버가 보낸 notice면 None
        nick: Option<String>,
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
    Kick {
        server_id: ServerId,
        channel: ChannelId,
        /// 강퇴당한 닉
        nick: String,
        /// 강퇴한 닉
        by: String,
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
        /// 토픽을 바꾼 닉 (TOPIC 커맨드). None이면 입장/조회 시 받은 현재 토픽(332/331)
        changed_by: Option<String>,
    },
    /// 채널 유저 목록 전체 (NAMES 응답). 권한 접두사는 제거된 닉 목록 — UI는 목록을 교체한다
    Names {
        server_id: ServerId,
        channel: ChannelId,
        users: Vec<String>,
    },
    Error {
        server_id: ServerId,
        message: String,
    },
}

#[derive(Debug, Clone)]
pub struct ChangeNickFailedPayload {
    pub server_id: ServerId,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum ServerStatusValue {
    Connecting,
    Connected,
    Registering,
    Disconnected,
    Disconnecting,
    Failed,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::mpsc::unbounded_channel;

    #[derive(Debug, Default)]
    struct CollectingSink(std::sync::Mutex<Vec<UiEvent>>);

    impl UiSink for CollectingSink {
        fn emit(&self, event: UiEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    fn status() -> ServerStatus {
        ServerStatus::Connected
    }

    #[tokio::test]
    async fn ui_sink_via_channel() {
        let (tx, mut rx) = unbounded_channel::<UiEvent>();
        let sink: EventSender = Arc::new(tx);
        let server_id = uuid::Uuid::now_v7();

        emit_server_status(&sink, server_id, status());
        emit_channel_lock_changed(&sink, server_id, "#test", true);
        emit_ui_event(&sink)
            .join(server_id, "#test".to_string(), "kim".to_string())
            .emit();

        let first = rx.recv().await.unwrap();
        let second = rx.recv().await.unwrap();
        let third = rx.recv().await.unwrap();

        assert!(matches!(first, UiEvent::ServerStatus(_)));
        assert!(matches!(second, UiEvent::ChannelLockChanged(_)));
        assert!(matches!(third, UiEvent::Ui(UIEventPayload::Join { .. })));
    }

    #[test]
    fn ui_sink_collect() {
        let inner = Arc::new(CollectingSink::default());
        let sink: EventSender = inner.clone();
        let server_id = uuid::Uuid::now_v7();

        emit_system_message(&sink, server_id, "hello");
        emit_change_nick_failed(&sink, server_id, "nope");

        assert_eq!(inner.0.lock().unwrap().len(), 2);
    }

    #[test]
    fn missing_payload_is_logged_only() {
        let inner = Arc::new(CollectingSink::default());
        let sink: EventSender = inner.clone();
        let _server_id = uuid::Uuid::now_v7();

        // payload 없이 emit → 큐잉만 안 되고 패닉/오류 아님
        emit_ui_event(&sink).emit();
        assert!(inner.0.lock().unwrap().is_empty());
    }
}
