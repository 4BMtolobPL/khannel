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
pub enum ServerRuntime {
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

    pub async fn graceful_shutdown(self) {
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

pub struct ServerState {
    runtime: Mutex<ServerRuntime>,
    config: Mutex<ServerConfig>,
    channels: Mutex<HashMap<ChannelId, ChannelState>>,
    /// 서버에 실제로 등록된 현재 닉 (런타임 — 강제 변경 포함). 접속할 때마다 다시 맞춘다.
    current_nickname: Mutex<String>,
    /// 사용자가 요청해 아직 반영되지 않은 새 닉. 이 닉으로 바뀌었을 때만 config에 저장한다
    /// (NickServ 강제 변경 등 요청과 무관한 변경은 저장하지 않는다).
    pending_nick: Mutex<Option<String>>,
}

impl ServerState {
    pub fn new(runtime: ServerRuntime, config: ServerConfig) -> Self {
        Self {
            runtime: Mutex::new(runtime),
            current_nickname: Mutex::new(config.nickname().to_string()),
            config: Mutex::new(config),
            channels: Mutex::new(HashMap::new()),
            pending_nick: Mutex::new(None),
        }
    }

    pub fn with_channel(config: ServerConfig, channels: HashMap<ChannelId, ChannelState>) -> Self {
        Self {
            runtime: Mutex::new(ServerRuntime::Disconnected),
            current_nickname: Mutex::new(config.nickname().to_string()),
            config: Mutex::new(config),
            channels: Mutex::new(channels),
            pending_nick: Mutex::new(None),
        }
    }

    pub fn status(&self) -> ServerStatus {
        self.runtime.lock().unwrap().status()
    }

    /// 서버가 Failed 상태에 빠진 마지막 사유 (UI의 재연결 실패 표시 용도).
    pub fn last_error(&self) -> Option<String> {
        match &*self.runtime.lock().unwrap() {
            ServerRuntime::Failed { error } => Some(error.clone()),
            _ => None,
        }
    }

    pub fn config(&self) -> ServerConfig {
        self.config.lock().unwrap().clone()
    }

    pub fn channels(&self) -> HashMap<ChannelId, ChannelState> {
        self.channels.lock().unwrap().clone()
    }

    pub fn insert_channel(&self, channel_name: &str, locked: bool) {
        self.channels.lock().unwrap().insert(
            channel_name.to_string(),
            ChannelState {
                name: channel_name.to_string(),
                locked,
            },
        );
    }

    pub fn remove_channel(&self, channel_name: &str) -> Option<ChannelState> {
        self.channels.lock().unwrap().remove(channel_name)
    }

    pub fn current_nickname(&self) -> String {
        self.current_nickname.lock().unwrap().clone()
    }

    pub fn set_current_nickname(&self, new_nick: &str) {
        *self.current_nickname.lock().unwrap() = new_nick.to_string();
    }

    /// 사용자가 닉 변경을 요청했다 (NICK 전송 직후).
    pub fn request_nick_change(&self, new_nick: &str) {
        *self.pending_nick.lock().unwrap() = Some(new_nick.to_string());
    }

    /// 닉 변경 요청이 실패했다 — 대기 중인 요청을 버린다.
    pub fn cancel_nick_change(&self) {
        self.pending_nick.lock().unwrap().take();
    }

    /// 내 닉이 `new_nick`으로 바뀌었다. 요청한 닉이면(대소문자 무시) config의 접속 닉도
    /// 갱신하고 true를 돌려준다 — 호출자가 스냅샷을 저장한다. 요청과 무관한 변경이면 false.
    pub fn complete_nick_change(&self, new_nick: &str) -> bool {
        let mut pending = self.pending_nick.lock().unwrap();
        let requested = pending
            .as_deref()
            .is_some_and(|nick| nick.eq_ignore_ascii_case(new_nick));
        if requested {
            pending.take();
            self.config.lock().unwrap().set_nickname(new_nick);
        }
        requested
    }

    pub fn is_active(&self) -> bool {
        matches!(
            &*self.runtime.lock().unwrap(),
            ServerRuntime::Connecting { .. }
                | ServerRuntime::Registering { .. }
                | ServerRuntime::Connected { .. }
        )
    }

    pub fn is_channel_locked(&self, channel: &str) -> bool {
        self.channels
            .lock()
            .unwrap()
            .get(channel)
            .map(|s| s.locked)
            .unwrap_or(false)
    }

    /// 채널 항목 존재 여부 (재조인 시 lock을 덮어쓰지 않기 위한 용도)
    pub fn contains_channel(&self, channel_name: &str) -> bool {
        self.channels.lock().unwrap().contains_key(channel_name)
    }

    pub fn set_channel_locked(&self, channel: &str, locked: bool) {
        if let Some(channel) = self.channels.lock().unwrap().get_mut(channel) {
            channel.locked = locked;
        }
    }

    pub fn send_command(&self, cmd: ServerCommand) -> anyhow::Result<()> {
        let guard = self.runtime.lock().unwrap();
        match &*guard {
            ServerRuntime::Connected { tx, .. } | ServerRuntime::Registering { tx, .. } => {
                debug!(command = %cmd, "tx send");
                tx.send(cmd).map_err(|e| anyhow!("Failed to send: {}", e))
            }
            _ => Err(anyhow!("Server not connected")),
        }
    }

    /// 연결 시작: Disconnected/Failed일 때만 `spawn`(액터 기동)을 실행하고 Connecting으로 전이한다.
    ///
    /// spawn을 runtime 락 안에서 수행하므로 액터의 다음 전이(registering/failed)는 반드시
    /// Connecting 확정 이후에 일어난다. (spawn 후 전이하던 이전 구현은 빠른 연결 실패 시
    /// Failed를 `Connecting{종료된 handle}`로 덮어써 재연결이 영구 거부됐다.)
    /// 이미 활성/Disconnecting 상태면 spawn하지 않고 false를 반환한다.
    pub fn start_connecting(&self, spawn: impl FnOnce() -> JoinHandle<()>) -> bool {
        let mut guard = self.runtime.lock().unwrap();
        match &*guard {
            ServerRuntime::Disconnected | ServerRuntime::Failed { .. } => {
                *guard = ServerRuntime::Connecting { handle: spawn() };
                true
            }
            _ => false,
        }
    }

    /// 상태가 Connecting이 아니면(예: 취소됨) 현재 상태를 유지하고 tx는 버린다.
    pub fn transition_to_registering(&self, tx: UnboundedSender<ServerCommand>) {
        let mut guard = self.runtime.lock().unwrap();
        match std::mem::take(&mut *guard) {
            ServerRuntime::Connecting { handle } => {
                *guard = ServerRuntime::Registering { tx, handle };
            }
            other => *guard = other,
        }
    }

    /// 상태가 Registering이 아니면(예: Disconnecting) 현재 상태를 유지한다.
    pub fn transition_to_connected(&self) {
        let mut guard = self.runtime.lock().unwrap();
        match std::mem::take(&mut *guard) {
            ServerRuntime::Registering { tx, handle } => {
                *guard = ServerRuntime::Connected { tx, handle };
            }
            other => *guard = other,
        }
    }

    pub fn transition_to_disconnected(&self) {
        *self.runtime.lock().unwrap() = ServerRuntime::Disconnected;
    }

    pub fn transition_to_failed(&self, error: String) {
        *self.runtime.lock().unwrap() = ServerRuntime::Failed { error };
    }

    pub fn disconnect(&self) {
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

    pub fn abort_connecting(&self) -> bool {
        let mut guard = self.runtime.lock().unwrap();
        if let ServerRuntime::Connecting { handle } = std::mem::take(&mut *guard) {
            handle.abort();
            *guard = ServerRuntime::Disconnected;
            true
        } else {
            false
        }
    }

    pub fn take_runtime(&self) -> ServerRuntime {
        std::mem::take(&mut *self.runtime.lock().unwrap())
    }
}

impl Originator<ServerStateSnapshot> for ServerState {
    fn snapshot(&self) -> ServerStateSnapshot {
        ServerStateSnapshot::new(self.config(), self.channels())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_server() -> ServerState {
        ServerState::new(
            ServerRuntime::Disconnected,
            ServerConfig::new("irc.test".to_string(), 6667, false, "nick".to_string()),
        )
    }

    #[test]
    fn channel_lock_round_trip() {
        let server = test_server();

        // 본문 채널명 키 기준으로 들어가고(복합 ID 아님), lock 토글이 반영된다.
        server.insert_channel("#test", false);
        assert!(!server.is_channel_locked("#test"));

        server.set_channel_locked("#test", true);
        assert!(server.is_channel_locked("#test"));

        server.set_channel_locked("#test", false);
        assert!(!server.is_channel_locked("#test"));

        // 없는 채널 키에 대한 조회/설정은 no-op(false)
        assert!(!server.is_channel_locked("#missing"));
        server.set_channel_locked("#missing", true);
        assert!(!server.is_channel_locked("#missing"));
    }

    #[test]
    fn only_requested_nick_change_updates_config() {
        let server = test_server();

        // 요청 없는 (강제) 변경은 config를 바꾸지 않는다
        assert!(!server.complete_nick_change("Guest123"));
        assert_eq!(server.config().nickname(), "nick");

        // 요청한 닉으로 바뀌면(대소문자 무시) config 갱신, 요청은 소진
        server.request_nick_change("kim");
        assert!(!server.complete_nick_change("Guest123"));
        assert!(server.complete_nick_change("Kim"));
        assert_eq!(server.config().nickname(), "Kim");
        assert!(!server.complete_nick_change("Kim"));

        // 실패로 취소된 요청은 이후 같은 닉으로 바뀌어도 저장하지 않는다
        server.request_nick_change("lee");
        server.cancel_nick_change();
        assert!(!server.complete_nick_change("lee"));
        assert_eq!(server.config().nickname(), "Kim");
    }

    #[test]
    fn contains_channel_for_rejoin_preserves_lock() {
        let server = test_server();
        server.insert_channel("#test", true);

        assert!(server.contains_channel("#test"));
        // 기존 항목이 있으면 join_channel은 insert하지 않는다 → lock 유지가 전제가 된다.
        assert!(server.is_channel_locked("#test"));
        assert!(!server.contains_channel("#missing"));
    }

    #[tokio::test]
    async fn start_connecting_only_from_idle_states() {
        let server = test_server();

        assert!(server.start_connecting(|| tokio::spawn(async {})));
        assert_eq!(server.status(), ServerStatus::Connecting);

        // 활성 상태에서는 spawn 자체가 일어나지 않는다
        let mut spawned = false;
        assert!(!server.start_connecting(|| {
            spawned = true;
            tokio::spawn(async {})
        }));
        assert!(!spawned);

        // Failed에서는 재시작 가능
        server.transition_to_failed("refused".to_string());
        assert!(server.start_connecting(|| tokio::spawn(async {})));
        assert_eq!(server.status(), ServerStatus::Connecting);
    }

    #[tokio::test]
    async fn start_connecting_refused_while_disconnecting() {
        let server = test_server();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        assert!(server.start_connecting(|| tokio::spawn(async {})));
        server.transition_to_registering(tx);
        server.disconnect();
        assert_eq!(server.status(), ServerStatus::Disconnecting);

        // 이전 액터가 아직 끝나지 않았으므로 새 액터를 띄우면 안 된다
        // (띄우면 이전 액터의 종료 전이가 새 세션 상태를 덮어쓴다)
        assert!(!server.start_connecting(|| tokio::spawn(async {})));
        assert_eq!(server.status(), ServerStatus::Disconnecting);
    }

    #[tokio::test]
    async fn mismatched_transitions_keep_current_state() {
        let server = test_server();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        // Disconnected에서 registering/connected 전이는 no-op
        server.transition_to_registering(tx.clone());
        assert_eq!(server.status(), ServerStatus::Disconnected);
        server.transition_to_connected();
        assert_eq!(server.status(), ServerStatus::Disconnected);

        // Disconnecting 중 늦게 도착한 RPL_WELCOME은 Disconnecting을 덮어쓰지 않는다
        // (이전 구현은 Disconnected로 바꾸며 handle을 유실했다)
        assert!(server.start_connecting(|| tokio::spawn(async {})));
        server.transition_to_registering(tx);
        server.disconnect();
        server.transition_to_connected();
        assert_eq!(server.status(), ServerStatus::Disconnecting);
    }

    /// 액터가 락 대기 없이 곧바로 실패를 기록하려 해도 Connecting 확정 이후에 반영된다.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn immediate_actor_failure_ends_in_failed() {
        let server = std::sync::Arc::new(test_server());
        let actor_server = server.clone();

        assert!(server.start_connecting(move || {
            tokio::spawn(async move {
                actor_server.transition_to_failed("refused".to_string());
            })
        }));

        tokio::time::timeout(Duration::from_secs(5), async {
            while server.status() != ServerStatus::Failed {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("failure was clobbered by Connecting");
        assert_eq!(server.last_error().as_deref(), Some("refused"));
    }
}
