//! IrcModel: ircService.ts + ircStore의 Rust 포팅.
//!
//! 이벤트 버스(UiEventReceiver)를 소비하며 GPUI Entity로 상태를 유지.
//! 이벤트 → 상태 반영 로직은 ircService.ts의 switch 문을 1:1 이식.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use gpui::Context;
use kirc_core::kirc::event::{
    ChangeNickFailedPayload, ChannelLockChangedEvent, ServerDetail, ServerStatusPayload,
    SystemMessagePayload, UIEventPayload, UiEvent,
};
use kirc_core::kirc::types::ServerId;
use serde::Serialize;

use crate::api::{UiApi, UiEventReceiver};

// ==================================== Types ====================================

pub type MessageId = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MessageType {
    User,
    System,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub kind: MessageType,
    pub id: MessageId,
    pub nickname: Option<String>,
    pub content: String,
    pub timestamp: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum UiServerStatus {
    Connecting,
    Connected,
    Registering,
    Disconnected,
    Disconnecting,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Server {
    pub id: ServerId,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub nickname: String,
    pub status: UiServerStatus,
    pub server_messages: Vec<ChatMessage>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Channel {
    pub server_id: ServerId,
    pub name: String,
    pub users: BTreeSet<String>,
    pub unread: u32,
    pub locked: bool,
    pub last_read_message_id: Option<MessageId>,
    pub topic: Option<String>,
}

fn map_status(status: &kirc_core::kirc::types::ServerStatus) -> UiServerStatus {
    use kirc_core::kirc::types::ServerStatus as S;
    match status {
        S::Connecting => UiServerStatus::Connecting,
        S::Connected => UiServerStatus::Connected,
        S::Registering => UiServerStatus::Registering,
        S::Disconnected => UiServerStatus::Disconnected,
        S::Disconnecting => UiServerStatus::Disconnecting,
        S::Failed => UiServerStatus::Failed,
    }
}

fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis() as u64
}

// ==================================== IrcModel ====================================

pub struct IrcModel {
    servers: HashMap<ServerId, Server>,
    channels: std::collections::HashMap<String, Channel>,
    messages: std::collections::HashMap<String, Vec<ChatMessage>>,
    current_server_id: Option<ServerId>,
    current_channel_id: Option<String>,
    nick_errors: HashMap<ServerId, String>,
    // 닉네임 변경 성공 캐시 — 닉변 모달의 성공 자동 닫힘 용도.
    nick_success: HashMap<ServerId, String>,
    api: Arc<UiApi>,
}

impl IrcModel {
    pub fn start(api: Arc<UiApi>, rx: UiEventReceiver, cx: &mut Context<Self>) -> Self {
        let this = Self {
            servers: HashMap::new(),
            channels: HashMap::new(),
            messages: HashMap::new(),
            current_server_id: None,
            current_channel_id: None,
            nick_errors: HashMap::new(),
            nick_success: HashMap::new(),
            api: api.clone(),
        };

        // 스냅샷 서버 동기화 (spawn: 비동기 컨텍스트에서 접근하기 위함)
        cx.spawn(async move |this, cx| {
            api.init();
            // 서버 목록은 스냅샷 복원 시점에 이미 kirc_state가 갖고 있으므로 즉시 동기화.
            // 상태 전이는 ServerStatus 이벤트가 이벤트 루프를 통해 실시간 반영한다.
            let servers = api.get_servers();
            let _ = this.update(cx, |m, cx| {
                for info in servers {
                    m.on_server_added(ServerDetail {
                        server_id: info.id,
                        host: info.host.clone(),
                        port: info.port,
                        tls: info.tls,
                        nickname: info.nickname.clone(),
                        status: info.status.clone(),
                    });
                }
                cx.notify();
            });
        })
        .detach();

        // UiEvent 소비 루프
        cx.spawn(async move |this, cx| {
            let mut rx = rx;
            while let Some(event) = rx.recv().await {
                let _ = this.update(cx, |m, cx| m.handle_event(event, cx));
            }
        })
        .detach();

        this
    }

    // ========================= Event Handlers =========================

    fn handle_event(&mut self, event: UiEvent, cx: &mut Context<Self>) {
        match event {
            UiEvent::ServerAdded(detail) => self.on_server_added(detail),
            UiEvent::ServerStatus(payload) => self.on_server_status(payload),
            UiEvent::ChannelLockChanged(payload) => self.on_channel_lock_changed(payload),
            UiEvent::SystemMessage(payload) => self.on_system_message(payload),
            UiEvent::ChangeNickFailed(payload) => self.on_change_nick_failed(payload),
            UiEvent::Ui(payload) => self.on_ui_event(payload),
        }

        cx.notify();
    }

    fn on_server_added(&mut self, d: ServerDetail) {
        if self.servers.contains_key(&d.server_id) {
            return;
        }
        self.servers.insert(
            d.server_id,
            Server {
                id: d.server_id,
                name: d.host.clone(),
                host: d.host,
                port: d.port,
                tls: d.tls,
                nickname: d.nickname,
                status: map_status(&d.status),
                server_messages: Vec::new(),
            },
        );
    }

    fn on_server_status(&mut self, p: ServerStatusPayload) {
        if let Some(server) = self.servers.get_mut(&p.server_id) {
            server.status = map_status(&p.status);
        }
    }

    fn on_channel_lock_changed(&mut self, p: ChannelLockChangedEvent) {
        self.update_channel_lock(&p.channel, p.locked);
    }

    fn on_system_message(&mut self, p: SystemMessagePayload) {
        self.add_server_message(
            p.server_id,
            ChatMessage {
                kind: MessageType::System,
                id: uuid::Uuid::now_v7().to_string(),
                nickname: None,
                content: p.message,
                timestamp: now_ms(),
            },
        );
    }

    fn on_change_nick_failed(&mut self, p: ChangeNickFailedPayload) {
        self.nick_errors.insert(p.server_id, p.reason);
    }

    fn on_ui_event(&mut self, payload: UIEventPayload) {
        use UIEventPayload as P;
        match payload {
            P::UserMessage {
                server_id,
                channel,
                nick,
                content,
                timestamp,
            } => {
                self.ensure_channel(server_id, &channel);
                let channel_id = self.get_channel_id(&server_id, &channel);
                self.add_message(
                    server_id,
                    channel_id,
                    ChatMessage {
                        kind: MessageType::User,
                        id: uuid::Uuid::now_v7().to_string(),
                        nickname: Some(nick),
                        content,
                        timestamp,
                    },
                );
            }
            P::Join {
                server_id,
                channel,
                nick,
            } => {
                self.ensure_channel(server_id, &channel);
                let channel_id = self.get_channel_id(&server_id, &channel);
                if let Some(server) = self.servers.get(&server_id).cloned() {
                    let own_join = nick == server.nickname;

                    if let Some(channel) = self.channels.get_mut(&channel_id) {
                        channel.users.insert(nick.clone());

                        // 내 Join이면 현재 채널로 자동 전환
                        if own_join {
                            self.current_server_id = Some(server_id);
                            self.current_channel_id = Some(channel_id.clone());
                        }

                        self.add_message(
                            server_id,
                            channel_id,
                            ChatMessage {
                                kind: MessageType::System,
                                id: uuid::Uuid::now_v7().to_string(),
                                nickname: None,
                                content: format!("{} joined the channel", nick),
                                timestamp: now_ms(),
                            },
                        );
                    }
                }
            }
            P::Part {
                server_id,
                channel,
                nick,
                ..
            } => {
                let server = self.servers.get(&server_id).cloned();
                if let Some(server) = server {
                    let channel_id = self.get_channel_id(&server_id, &channel);
                    if nick == server.nickname {
                        // 내가 나간 경우
                        if self.current_channel_id.as_ref() == Some(&channel_id) {
                            self.current_channel_id = None;
                        }
                        self.channels.remove(&channel_id);
                    } else if let Some(channel) = self.channels.get_mut(&channel_id) {
                        // 다른 사람이 나간 경우
                        channel.users.remove(&nick);

                        self.add_message(
                            server_id,
                            channel_id,
                            ChatMessage {
                                kind: MessageType::System,
                                id: uuid::Uuid::now_v7().to_string(),
                                nickname: None,
                                content: format!("{} left the channel", nick),
                                timestamp: now_ms(),
                            },
                        );
                    }
                }
            }
            P::Quit {
                server_id,
                nick,
                reason,
            } => {
                if self.servers.contains_key(&server_id) {
                    let targets: Vec<String> = self
                        .channels
                        .values()
                        .filter(|c| c.server_id == server_id && c.users.contains(&nick))
                        .map(|c| c.name.clone())
                        .collect();

                    for channel_name in targets {
                        let channel_id = self.get_channel_id(&server_id, &channel_name);
                        if let Some(channel) = self.channels.get_mut(&channel_id) {
                            channel.users.remove(&nick);

                            let suffix = reason
                                .clone()
                                .map(|r| format!(" ({})", r))
                                .unwrap_or_default();
                            self.add_message(
                                server_id,
                                channel_id,
                                ChatMessage {
                                    kind: MessageType::System,
                                    id: uuid::Uuid::now_v7().to_string(),
                                    nickname: None,
                                    content: format!("{} quit{}", nick, suffix),
                                    timestamp: now_ms(),
                                },
                            );
                        }
                    }
                }
            }
            P::Nick {
                server_id,
                old_nick,
                new_nick,
            } => {
                if let Some(server) = self.servers.get_mut(&server_id) {
                    if server.nickname == old_nick {
                        server.nickname = new_nick.clone();
                        self.nick_success.insert(server_id, new_nick.clone());
                    }
                }
                let targets: Vec<String> = self
                    .channels
                    .values()
                    .filter_map(|c| {
                        if c.server_id == server_id && c.users.contains(&old_nick) {
                            Some(c.name.clone())
                        } else {
                            None
                        }
                    })
                    .collect();

                for channel_name in targets {
                    let channel_id = self.get_channel_id(&server_id, &channel_name);
                    if let Some(channel) = self.channels.get_mut(&channel_id) {
                        channel.users.remove(&old_nick);
                        channel.users.insert(new_nick.clone());

                        self.add_message(
                            server_id,
                            channel_id,
                            ChatMessage {
                                kind: MessageType::System,
                                id: uuid::Uuid::now_v7().to_string(),
                                nickname: None,
                                content: format!("{} is now known as {}", old_nick, new_nick),
                                timestamp: now_ms(),
                            },
                        );
                    }
                }
            }
            P::Topic {
                server_id,
                channel,
                topic,
            } => {
                self.ensure_channel(server_id, &channel);
                if let Some(server) = self.servers.get(&server_id).cloned() {
                    let channel_id = self.get_channel_id(&server_id, &channel);
                    if let Some(channel) = self.channels.get_mut(&channel_id) {
                        channel.topic = topic.clone();

                        self.add_message(
                            server_id,
                            channel_id,
                            ChatMessage {
                                kind: MessageType::System,
                                id: uuid::Uuid::now_v7().to_string(),
                                nickname: None,
                                content: format!("Topic set to: {}", topic.unwrap_or_default()),
                                timestamp: now_ms(),
                            },
                        );
                    }
                    let _ = server;
                }
            }
            P::Error { server_id, message } => {
                self.add_server_message(
                    server_id,
                    ChatMessage {
                        kind: MessageType::System,
                        id: uuid::Uuid::now_v7().to_string(),
                        nickname: None,
                        content: format!("Error: {}", message),
                        timestamp: now_ms(),
                    },
                );
            }
        }
    }

    // ========================= Store Operations =========================

    pub fn ensure_channel(&mut self, server_id: ServerId, channel_name: &str) {
        if !self.servers.contains_key(&server_id) {
            return;
        }
        let channel_id = self.get_channel_id(&server_id, channel_name);

        if !self.channels.contains_key(&channel_id) {
            self.channels.insert(
                channel_id.clone(),
                Channel {
                    server_id,
                    name: channel_name.to_string(),
                    users: BTreeSet::new(),
                    unread: 0,
                    locked: false,
                    last_read_message_id: None,
                    topic: None,
                },
            );
        }

        self.messages.entry(channel_id).or_default();
    }

    pub fn add_message(&mut self, server_id: ServerId, channel_id: String, message: ChatMessage) {
        if !self.channels.contains_key(&channel_id) {
            return;
        }

        let channel_unread = self
            .channels
            .get(&channel_id)
            .map(|c| c.unread)
            .unwrap_or(0);

        if let Some(list) = self.messages.get_mut(&channel_id) {
            list.push(message.clone());
        }

        let is_current = self.current_server_id == Some(server_id)
            && self.current_channel_id.as_ref() == Some(&channel_id);

        // 현재 채널이 아니면 unread 증가
        if !is_current && message.kind == MessageType::User {
            if let Some(channel) = self.channels.get_mut(&channel_id) {
                channel.unread = channel_unread + 1;
            }
        }
    }

    pub fn remove_unread_message(&mut self, channel_id: &str) {
        if let Some(channel) = self.channels.get_mut(channel_id) {
            channel.unread = 0;
        }
    }

    pub fn add_server_message(&mut self, server_id: ServerId, message: ChatMessage) {
        if let Some(server) = self.servers.get_mut(&server_id) {
            server.server_messages.push(message);
        }
    }

    pub fn update_channel_lock(&mut self, channel_id: &str, locked: bool) {
        if let Some(channel) = self.channels.get_mut(channel_id) {
            channel.locked = locked;
        }
    }

    // Phase 3 예약: MessageView 자동 스크롤 / UnreadDivider 로직에서 사용.
    // 값이 실제로 변경될 때만 notify한다(렌더 중 호출해도 무한 루프 없음).
    pub fn update_channel_last_read_message_id(
        &mut self,
        channel_id: &str,
        last_read_message_id: Option<MessageId>,
        cx: &mut Context<Self>,
    ) {
        if let Some(channel) = self.channels.get_mut(channel_id) {
            if channel.last_read_message_id != last_read_message_id {
                channel.last_read_message_id = last_read_message_id;
                cx.notify();
            }
        }
    }

    // ========================= Phase 3: 모달/컨텍스트 메뉴 =========================

    /// 새 서버 연결 (ServerModal 제출). server_id는 코어가 발급한다.
    pub fn add_server(
        &mut self,
        host: String,
        port: u16,
        tls: bool,
        nickname: String,
        cx: &mut Context<Self>,
    ) {
        let payload =
            kirc_core::kirc::commands::ConnectServerPayload::new(None, host, port, tls, nickname);
        if let Err(e) = self.api.connect_server(payload) {
            tracing::warn!("connect_server failed: {}", e);
        }
        cx.notify();
    }

    /// 채널 입장 (ChannelJoinModal 제출, 채널 컨텍스트 메뉴 Join).
    pub fn join_channel(&mut self, server_id: ServerId, channel: &str, cx: &mut Context<Self>) {
        if let Err(e) = self.api.join_channel(server_id, channel) {
            tracing::warn!("join_channel failed: {}", e);
        }
        cx.notify();
    }

    /// 닉변 실패 에러 조회 (ChangeNickModal 표시용).
    pub fn nick_error(&self, server_id: &ServerId) -> Option<&str> {
        self.nick_errors.get(server_id).map(String::as_str)
    }

    /// 닉변 실패 에러 제거 (모달 열림/닫힘/재제출 시 — Svelte nickErrors.delete 대응).
    pub fn clear_nick_error(&mut self, server_id: &ServerId, cx: &mut Context<Self>) {
        if self.nick_errors.remove(server_id).is_some() {
            cx.notify();
        }
    }

    /// 닉변 성공 캐시를 꺼낸다 (ChangeNickModal 성공 자동 닫힘 — Svelte nickSuccess 대응).
    pub fn take_nick_success(&mut self, server_id: &ServerId) -> Option<String> {
        self.nick_success.remove(server_id)
    }

    // ========================= State Mutation (UI 액션) =========================

    pub fn select_server(&mut self, server_id: ServerId, cx: &mut Context<Self>) {
        self.current_server_id = Some(server_id);
        self.current_channel_id = None;
        cx.notify();
    }

    pub fn set_current_channel(&mut self, channel_id: Option<String>, cx: &mut Context<Self>) {
        self.current_channel_id = channel_id.clone();
        if let Some(channel) = channel_id {
            self.remove_unread_message(&channel);
        }
        cx.notify();
    }

    pub fn get_channel_id(&self, server_id: &ServerId, channel_name: &str) -> String {
        format!("{}:{}", server_id, channel_name)
    }

    // ========================= Queries for UI =========================

    pub fn servers(&self) -> &HashMap<ServerId, Server> {
        &self.servers
    }

    pub fn channels(&self) -> &HashMap<String, Channel> {
        &self.channels
    }

    pub fn current_server_id(&self) -> Option<&ServerId> {
        self.current_server_id.as_ref()
    }

    pub fn current_channel_id(&self) -> Option<&str> {
        self.current_channel_id.as_deref()
    }

    // Phase 3 예약: 모달/입력창 헤더(서버 표시명)에서 사용.
    pub fn current_server(&self) -> Option<&Server> {
        self.current_server_id
            .as_ref()
            .and_then(|id| self.servers.get(id))
    }

    pub fn current_channel(&self) -> Option<&Channel> {
        self.current_channel_id
            .as_ref()
            .and_then(|id| self.channels.get(id))
    }

    pub fn current_messages(&self) -> Option<&Vec<ChatMessage>> {
        self.current_channel_id
            .as_ref()
            .and_then(|id| self.messages.get(id))
    }

    pub fn is_locked(&self) -> bool {
        self.current_channel().map(|c| c.locked).unwrap_or(false)
    }

    pub fn server_unread(&self) -> HashMap<ServerId, u32> {
        let mut result: HashMap<ServerId, u32> =
            self.servers.keys().map(|id| (*id, 0u32)).collect();
        for channel in self.channels.values() {
            *result.entry(channel.server_id).or_insert(0) += channel.unread;
        }
        result
    }

    // ========================= UI 액션 (명령 전송) =========================

    pub fn send_message(&mut self, text: &str, cx: &mut Context<Self>) {
        let Some(server_id) = self.current_server_id else {
            return;
        };
        if text.trim().is_empty() || self.is_locked() {
            return;
        }
        let Some(target) = self.current_channel().map(|c| c.name.clone()) else {
            return;
        };

        if let Err(e) = self.api.send_message(server_id, &target, text) {
            tracing::warn!("send_message failed: {}", e);
        }
        cx.notify();
    }

    pub fn toggle_lock(&mut self, cx: &mut Context<Self>) {
        let Some(server_id) = self.current_server_id else {
            return;
        };
        let Some(channel_name) = self.current_channel_id.clone() else {
            return;
        };

        let payload = kirc_core::kirc::commands::ChannelPayload::new(server_id, channel_name);
        let result = if self.is_locked() {
            self.api.unlock_channel(&payload)
        } else {
            self.api.lock_channel(&payload)
        };

        if let Err(e) = result {
            tracing::warn!("toggle_lock failed: {}", e);
        }
        cx.notify();
    }

    // 채널 컨텍스트 메뉴(Leave)에서 사용.
    pub fn leave_channel(&mut self, channel_id: &str, cx: &mut Context<Self>) {
        let Some(channel) = self.channels.get(channel_id) else {
            return;
        };
        let payload =
            kirc_core::kirc::commands::ChannelPayload::new(channel.server_id, channel.name.clone());

        if let Err(e) = self.api.leave_channel(&payload) {
            tracing::warn!("leave_channel failed: {}", e);
        }
        cx.notify();
    }

    // 서버 컨텍스트 메뉴(Disconnect)에서 사용.
    pub fn disconnect_server(&mut self, server_id: ServerId, cx: &mut Context<Self>) {
        if let Err(e) = self.api.disconnect_server(server_id) {
            tracing::warn!("disconnect_server failed: {}", e);
        }
        cx.notify();
    }

    // 서버 컨텍스트 메뉴(Connect)에서 사용.
    pub fn connect_server(&mut self, server_id: ServerId, cx: &mut Context<Self>) {
        // 기존 서버 재연결 (스냅샷 서버)
        let Some(server) = self.servers.get(&server_id).cloned() else {
            return;
        };

        let payload = kirc_core::kirc::commands::ConnectServerPayload::new(
            Some(server_id),
            server.host.clone(),
            server.port,
            server.tls,
            server.nickname.clone(),
        );
        if let Err(e) = self.api.connect_server(payload) {
            tracing::warn!("connect_server failed: {}", e);
        }
        cx.notify();
    }

    // ChangeNicknameModal 제출에서 사용.
    pub fn change_nickname(&mut self, server_id: ServerId, new_nick: &str, cx: &mut Context<Self>) {
        let payload =
            kirc_core::kirc::commands::ChangeNickPayload::new(server_id, new_nick.to_string());
        if let Err(e) = self.api.change_nickname(&payload) {
            tracing::warn!("change_nickname failed: {}", e);
        }
        cx.notify();
    }
}
