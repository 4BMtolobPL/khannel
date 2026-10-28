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

/// 사이드바 채널 표시 순서: 채널명 오름차순.
/// IRC 채널명은 대소문자를 구분하지 않으므로 소문자 기준으로 먼저 비교하고,
/// 같으면 원래 이름으로 비교해 순서를 결정적으로 만든다.
fn channel_order(a: &Channel, b: &Channel) -> std::cmp::Ordering {
    a.name
        .to_lowercase()
        .cmp(&b.name.to_lowercase())
        .then_with(|| a.name.cmp(&b.name))
}

/// 사이드바 서버 표시 순서: 서버 이름 오름차순(대소문자 무시), 같으면 ID 순.
fn server_order(a: &Server, b: &Server) -> std::cmp::Ordering {
    a.name
        .to_lowercase()
        .cmp(&b.name.to_lowercase())
        .then_with(|| a.name.cmp(&b.name))
        .then_with(|| a.id.cmp(&b.id))
}

/// IRC 닉 비교 — 대소문자를 구분하지 않는다.
pub fn nick_eq(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// 채널 이름인지(IRC 채널 접두사 `#&+!`). 아니면 닉 대상(DM/query)이다.
pub fn is_channel_name(name: &str) -> bool {
    name.starts_with(['#', '&', '+', '!'])
}

/// UserMessage를 쌓을 버퍼 이름.
/// 나에게 온 DM은 target이 내 닉이므로 보낸 사람 닉 버퍼에 쌓는다 → 그 버퍼에서 답장하면
/// target이 상대 닉이 되고, 내 답장 에코(target=상대 닉)도 같은 버퍼로 모인다.
fn message_buffer_name(target: &str, sender: &str, own_nickname: Option<&str>) -> String {
    let to_me = !is_channel_name(target) && own_nickname.is_some_and(|own| nick_eq(target, own));
    if to_me {
        sender.to_string()
    } else {
        target.to_string()
    }
}

fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis() as u64
}

// ==================================== 닉변 요청 추적 ====================================

/// 닉네임 변경 요청 ↔ 성공 매칭.
///
/// 내 닉 변경 이벤트(NICK)는 서비스의 강제 변경(예: NickServ → Guest1234) 등 요청과 무관하게도
/// 온다. 요청한 닉으로 실제 바뀌었을 때만 성공으로 기록해, 닉변 모달이 엉뚱한 이벤트에
/// 닫히지 않게 한다. (이전: 모든 내 닉 변경을 성공으로 쌓고 모달을 열 때도 지우지 않아, 남은
/// 기록 때문에 모달이 열린 직후 첫 모델 변경에 닫혔다.)
#[derive(Default)]
struct NickChangeTracker {
    /// 서버별 요청 중인 새 닉
    pending: HashMap<ServerId, String>,
    /// 서버별 요청이 반영된 닉 (모달이 꺼내 가면 제거)
    succeeded: HashMap<ServerId, String>,
}

impl NickChangeTracker {
    fn request(&mut self, server_id: ServerId, new_nick: &str) {
        self.pending.insert(server_id, new_nick.to_string());
        self.succeeded.remove(&server_id);
    }

    fn on_own_nick_changed(&mut self, server_id: ServerId, new_nick: &str) {
        // IRC 닉은 대소문자를 구분하지 않는다
        let requested = self
            .pending
            .get(&server_id)
            .is_some_and(|pending| pending.eq_ignore_ascii_case(new_nick));
        if requested {
            self.pending.remove(&server_id);
            self.succeeded.insert(server_id, new_nick.to_string());
        }
    }

    fn on_failed(&mut self, server_id: ServerId) {
        self.pending.remove(&server_id);
    }

    fn take_success(&mut self, server_id: ServerId) -> Option<String> {
        self.succeeded.remove(&server_id)
    }

    fn reset(&mut self, server_id: ServerId) {
        self.pending.remove(&server_id);
        self.succeeded.remove(&server_id);
    }
}

// ==================================== IrcModel ====================================

pub struct IrcModel {
    servers: HashMap<ServerId, Server>,
    channels: std::collections::HashMap<String, Channel>,
    messages: std::collections::HashMap<String, Vec<ChatMessage>>,
    current_server_id: Option<ServerId>,
    current_channel_id: Option<String>,
    nick_errors: HashMap<ServerId, String>,
    // 닉네임 변경 요청 ↔ 성공 매칭 — 닉변 모달의 성공 자동 닫힘 용도.
    nick_change: NickChangeTracker,
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
            nick_change: NickChangeTracker::default(),
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
                    // 채널/lock은 코어 스냅샷에서 기동 시 동기화한다.
                    // (이벤트만으론 복원 채널이 갱신되지 않아 lock이 사라져 보였다)
                    for channel in &info.channels {
                        m.sync_channel_from_core(info.id, channel.name(), channel.locked());
                    }
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
        // 코어 이벤트는 채널명을 담으므로 복합 ID(`serverId:channel`)로 조합해 갱신
        let channel_id = self.get_channel_id(&p.server_id, &p.channel);
        self.update_channel_lock(&channel_id, p.locked);
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
        self.nick_change.on_failed(p.server_id);
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
                let own_nickname = self.servers.get(&server_id).map(|s| s.nickname.as_str());
                let channel = message_buffer_name(&channel, &nick, own_nickname);
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
                    let own_join = nick_eq(&nick, &server.nickname);

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
                    if nick_eq(&nick, &server.nickname) {
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
                    if nick_eq(&server.nickname, &old_nick) {
                        server.nickname = new_nick.clone();
                        // 모달로 요청한 변경일 때만 성공 처리 (강제 변경 등은 무시)
                        self.nick_change.on_own_nick_changed(server_id, &new_nick);
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

    /// 기동 시 코어 스냅샷의 채널(과 lock 상태)을 UI로 동기화한다.
    /// 기존 항목이 있으면 lock만 갱신하고, 없으면 lock을 반영해 생성한다.
    pub fn sync_channel_from_core(
        &mut self,
        server_id: ServerId,
        channel_name: &str,
        locked: bool,
    ) {
        if !self.servers.contains_key(&server_id) {
            return;
        }
        let channel_id = self.get_channel_id(&server_id, channel_name);
        match self.channels.get_mut(&channel_id) {
            Some(channel) => {
                if channel.locked != locked {
                    channel.locked = locked;
                }
            }
            None => {
                self.channels.insert(
                    channel_id.clone(),
                    Channel {
                        server_id,
                        name: channel_name.to_string(),
                        users: BTreeSet::new(),
                        unread: 0,
                        locked,
                        last_read_message_id: None,
                        topic: None,
                    },
                );
                self.messages.entry(channel_id).or_default();
            }
        }
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
    ) -> Result<(), String> {
        let payload =
            kirc_core::kirc::commands::ConnectServerPayload::new(None, host, port, tls, nickname);
        let result = self.api.connect_server(payload).map_err(|e| {
            tracing::warn!("connect_server failed: {}", e);
            e.to_string()
        });
        cx.notify();
        result
    }

    /// 채널 입장 (ChannelJoinModal 제출, 채널 컨텍스트 메뉴 Join).
    /// 실패 사유를 돌려준다 (ChannelJoinModal이 에러로 표시).
    pub fn join_channel(
        &mut self,
        server_id: ServerId,
        channel: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let result = self.api.join_channel(server_id, channel).map_err(|e| {
            tracing::warn!("join_channel failed: {}", e);
            e.to_string()
        });
        cx.notify();
        result
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
        self.nick_change.take_success(*server_id)
    }

    /// 닉변 요청/성공 기록 초기화 (모달 열 때 — 이전에 남은 성공 기록으로 즉시 닫히지 않게).
    pub fn reset_nick_change(&mut self, server_id: &ServerId) {
        self.nick_change.reset(*server_id);
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

    /// 사이드바 표시 순서(서버 이름순)로 정렬된 서버 목록.
    pub fn sorted_servers(&self) -> Vec<&Server> {
        let mut servers: Vec<&Server> = self.servers.values().collect();
        servers.sort_by(|a, b| server_order(a, b));
        servers
    }

    /// 사이드바 표시 순서(채널명순)로 정렬된 특정 서버의 채널 목록.
    pub fn sorted_channels(&self, server_id: ServerId) -> Vec<&Channel> {
        let mut channels: Vec<&Channel> = self
            .channels
            .values()
            .filter(|c| c.server_id == server_id)
            .collect();
        channels.sort_by(|a, b| channel_order(a, b));
        channels
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

    /// 메시지 뷰에 표시할 목록: 채널이 선택돼 있으면 채널 메시지, 서버만 선택돼 있으면
    /// 서버 버퍼(연결 알림, IRC ERROR, 연결 실패 사유 등 `server_messages`).
    pub fn visible_messages(&self) -> Option<&[ChatMessage]> {
        if self.current_channel_id.is_some() {
            self.current_messages().map(Vec::as_slice)
        } else {
            self.current_server()
                .map(|server| server.server_messages.as_slice())
        }
    }

    /// `visible_messages`가 가리키는 버퍼의 식별자 (MessageView의 버퍼 전환 감지용).
    /// 채널은 복합 채널 ID, 서버 버퍼는 `server:{id}` — 서로 겹치지 않는다.
    pub fn visible_buffer_key(&self) -> Option<String> {
        match (&self.current_channel_id, &self.current_server_id) {
            (Some(channel_id), _) => Some(channel_id.clone()),
            (None, Some(server_id)) => Some(format!("server:{server_id}")),
            (None, None) => None,
        }
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
        // 코어의 채널 맵은 채널명 키 기준이므로 본문 채널명으로 전달해야 한다.
        // (복합 ID를 전달하면 set_channel_locked가 no-op이 된다 — Phase 4 정규화)
        let Some(channel_name) = self.current_channel().map(|c| c.name.clone()) else {
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

        // DM(query) 버퍼는 서버에 PART할 대상이 없으므로 로컬에서만 닫는다
        if !is_channel_name(&channel.name) {
            if self.current_channel_id.as_deref() == Some(channel_id) {
                self.current_channel_id = None;
            }
            self.channels.remove(channel_id);
            cx.notify();
            return;
        }

        let payload =
            kirc_core::kirc::commands::ChannelPayload::new(channel.server_id, channel.name.clone());

        if let Err(e) = self.api.leave_channel(&payload) {
            tracing::warn!("leave_channel failed: {}", e);
        }
        cx.notify();
    }

    // 서버 컨텍스트 메뉴(Disconnect)에서 사용.
    // 연결 시도 중(Connecting)이면 연결을 취소한다 — 코어 disconnect는 Connecting을 건드리지 않는다.
    pub fn disconnect_server(&mut self, server_id: ServerId, cx: &mut Context<Self>) {
        let connecting = self
            .servers
            .get(&server_id)
            .is_some_and(|s| s.status == UiServerStatus::Connecting);
        let result = if connecting {
            self.api.cancel_connect(server_id)
        } else {
            self.api.disconnect_server(server_id)
        };
        if let Err(e) = result {
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
        match self.api.change_nickname(&payload) {
            Ok(()) => self.nick_change.request(server_id, new_nick),
            Err(e) => tracing::warn!("change_nickname failed: {}", e),
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(name: &str) -> Channel {
        Channel {
            server_id: uuid::Uuid::nil(),
            name: name.to_string(),
            users: BTreeSet::new(),
            unread: 0,
            locked: false,
            last_read_message_id: None,
            topic: None,
        }
    }

    fn server(id: u128, name: &str) -> Server {
        Server {
            id: uuid::Uuid::from_u128(id),
            name: name.to_string(),
            host: name.to_string(),
            port: 6697,
            tls: true,
            nickname: "nick".to_string(),
            status: UiServerStatus::Disconnected,
            server_messages: Vec::new(),
        }
    }

    #[test]
    fn nick_comparison_is_case_insensitive() {
        assert!(nick_eq("Kim", "kim"));
        assert!(!nick_eq("kim", "kimm"));
    }

    #[test]
    fn dm_to_me_goes_to_sender_buffer() {
        // 상대가 나에게 보낸 DM → 상대 닉 버퍼
        assert_eq!(message_buffer_name("Kim", "lee", Some("kim")), "lee");
        // 내가 상대에게 보낸 DM의 에코 → 같은 상대 닉 버퍼
        assert_eq!(message_buffer_name("lee", "kim", Some("kim")), "lee");
        // 채널 메시지는 그대로
        assert_eq!(message_buffer_name("#rust", "lee", Some("kim")), "#rust");
        assert!(is_channel_name("&local") && !is_channel_name("lee"));
    }

    #[test]
    fn nick_change_succeeds_only_for_requested_nick() {
        let server = uuid::Uuid::from_u128(1);
        let mut tracker = NickChangeTracker::default();

        // 요청 없이 온 강제 변경은 성공이 아니다
        tracker.on_own_nick_changed(server, "Guest1234");
        assert_eq!(tracker.take_success(server), None);

        // 요청한 닉으로 바뀌면 성공 (대소문자 무시), 한 번 꺼내면 사라진다
        tracker.request(server, "kim");
        tracker.on_own_nick_changed(server, "Kim");
        assert_eq!(tracker.take_success(server).as_deref(), Some("Kim"));
        assert_eq!(tracker.take_success(server), None);
    }

    #[test]
    fn nick_change_other_nick_or_failure_is_not_success() {
        let server = uuid::Uuid::from_u128(1);
        let other = uuid::Uuid::from_u128(2);
        let mut tracker = NickChangeTracker::default();

        tracker.request(server, "kim");
        // 다른 닉으로 바뀌거나 다른 서버의 변경은 무시
        tracker.on_own_nick_changed(server, "Guest1234");
        tracker.on_own_nick_changed(other, "kim");
        assert_eq!(tracker.take_success(server), None);
        assert_eq!(tracker.take_success(other), None);

        // 실패하면 요청이 사라져 이후 같은 닉으로 바뀌어도 성공이 아니다
        tracker.on_failed(server);
        tracker.on_own_nick_changed(server, "kim");
        assert_eq!(tracker.take_success(server), None);
    }

    #[test]
    fn nick_change_reset_drops_stale_success() {
        let server = uuid::Uuid::from_u128(1);
        let mut tracker = NickChangeTracker::default();

        tracker.request(server, "kim");
        tracker.on_own_nick_changed(server, "kim");
        // 모달을 다시 열면(reset) 남아 있던 성공 기록으로 닫히지 않는다
        tracker.reset(server);
        assert_eq!(tracker.take_success(server), None);
    }

    #[test]
    fn channels_sort_by_name_case_insensitive() {
        let mut channels = [
            channel("#rust"),
            channel("#Zed"),
            channel("#gpui"),
            channel("#Rust"),
            channel("#a"),
        ];
        channels.sort_by(channel_order);
        let names: Vec<&str> = channels.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["#a", "#gpui", "#Rust", "#rust", "#Zed"]);
    }

    #[test]
    fn servers_sort_by_name_then_id() {
        let mut servers = [
            server(3, "libera.chat"),
            server(2, "irc.oftc.net"),
            server(1, "Libera.chat"),
            server(4, "irc.oftc.net"),
        ];
        servers.sort_by(server_order);
        let order: Vec<(u128, &str)> = servers
            .iter()
            .map(|s| (s.id.as_u128(), s.name.as_str()))
            .collect();
        assert_eq!(
            order,
            [
                (2, "irc.oftc.net"),
                (4, "irc.oftc.net"),
                (1, "Libera.chat"),
                (3, "libera.chat"),
            ]
        );
    }
}
