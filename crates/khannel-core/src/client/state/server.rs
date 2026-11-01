use crate::client::persistence::ServerStateSnapshot;
use crate::client::state::channel::ChannelState;
use crate::client::types::server::ServerConfig;
use crate::client::types::{ChannelId, ServerCommand, ServerStatus};
use crate::memento::Originator;
use anyhow::anyhow;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::{self, JoinHandle};
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
    /// 연결이 끊겨 다시 연결하기 전 대기 중. 액터 task는 살아 있고(재시도 루프), 커맨드 채널은 없다.
    Reconnecting {
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
            ServerRuntime::Reconnecting { .. } => ServerStatus::Reconnecting,
            ServerRuntime::Failed { .. } => ServerStatus::Failed,
        }
    }

    /// 액터 task의 handle (액터가 없는 Disconnected/Failed면 None)
    fn handle(&self) -> Option<&JoinHandle<()>> {
        match self {
            ServerRuntime::Connecting { handle }
            | ServerRuntime::Registering { handle, .. }
            | ServerRuntime::Connected { handle, .. }
            | ServerRuntime::Disconnecting { handle }
            | ServerRuntime::Reconnecting { handle } => Some(handle),
            ServerRuntime::Disconnected | ServerRuntime::Failed { .. } => None,
        }
    }

    /// 지금 runtime이 `owner` task(액터)의 것인지
    fn is_owned_by(&self, owner: task::Id) -> bool {
        self.handle().is_some_and(|handle| handle.id() == owner)
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

            // 대기/접속 단계엔 서버 세션이 없으니 QUIT 없이 abort로 끝낸다
            ServerRuntime::Connecting { handle } | ServerRuntime::Reconnecting { handle } => {
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

    /// 연결되어 있지 않을 때의 닉 변경: 접속 닉(config)과 런타임 닉을 함께 바꾼다.
    /// 다음 접속부터 이 닉으로 등록한다.
    pub fn set_nickname(&self, nickname: &str) {
        self.config.lock().unwrap().set_nickname(nickname);
        self.set_current_nickname(nickname);
        self.cancel_nick_change();
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
                | ServerRuntime::Reconnecting { .. }
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

    // ── 상태 전이 ──
    //
    // 모든 전이는 상태가 실제로 바뀌었을 때만 `notify(새 상태)`를 runtime 락 안에서 호출한다.
    // 호출자는 여기서 UI에 상태/안내를 보낸다 — 락 안에서 보내므로 사용자 조작과 액터의 전이가
    // 겹쳐도 UI가 받는 순서가 실제 전이 순서와 같다. (notify 안에서 runtime을 다시 잠그는
    // `status()` 등을 부르면 교착되니 주의)
    //
    // 액터 쪽 전이(`transition_to_*`)는 `owner`(액터 자신의 task id)가 지금 runtime의 handle과
    // 같을 때만 반영한다. tokio의 abort는 다음 await에서야 걸리므로, 사용자가 취소하거나
    // '지금 연결'로 액터를 바꾼 직후에도 이전 액터가 동기 코드를 조금 더 실행할 수 있다 —
    // 그 이전 액터가 새 상태를 덮어쓰거나 늦은 이벤트를 보내지 못하게 막는다.

    /// 연결 시작: Disconnected/Failed일 때만 `spawn`(액터 기동)을 실행하고 Connecting으로 전이한다.
    ///
    /// spawn을 runtime 락 안에서 수행하므로 액터의 다음 전이(registering/failed)는 반드시
    /// Connecting 확정 이후에 일어난다. (spawn 후 전이하던 이전 구현은 빠른 연결 실패 시
    /// Failed를 `Connecting{종료된 handle}`로 덮어써 재연결이 영구 거부됐다.)
    /// 재연결 대기(Reconnecting) 중이면 대기 중인 액터를 abort하고 새로 띄운다 (지금 연결).
    /// 이미 활성/Disconnecting 상태면 spawn하지 않고 false를 반환한다.
    pub fn start_connecting(
        &self,
        spawn: impl FnOnce() -> JoinHandle<()>,
        notify: impl FnOnce(ServerStatus),
    ) -> bool {
        let mut guard = self.runtime.lock().unwrap();
        match &*guard {
            ServerRuntime::Disconnected | ServerRuntime::Failed { .. } => {}
            ServerRuntime::Reconnecting { handle } => handle.abort(),
            _ => return false,
        }
        *guard = ServerRuntime::Connecting { handle: spawn() };
        notify(guard.status());
        true
    }

    /// 사용자 끊기. 세션이 있으면 QUIT을 보내고 Disconnecting(액터가 정리 후 Disconnected로),
    /// 접속 시도/재연결 대기 중이면 액터를 멈추고 바로 Disconnected. 그 밖의 상태면 그대로.
    pub fn disconnect(&self, notify: impl FnOnce(ServerStatus)) {
        let mut guard = self.runtime.lock().unwrap();
        match std::mem::take(&mut *guard) {
            ServerRuntime::Registering { tx, handle } | ServerRuntime::Connected { tx, handle } => {
                debug!(command = %ServerCommand::Quit, "tx send");
                let _ = tx.send(ServerCommand::Quit);
                *guard = ServerRuntime::Disconnecting { handle };
            }
            // 접속 시도/재연결 대기 중엔 서버 세션이 없으므로 그대로 멈춘다
            ServerRuntime::Connecting { handle } | ServerRuntime::Reconnecting { handle } => {
                handle.abort();
                *guard = ServerRuntime::Disconnected;
            }
            other => {
                *guard = other;
                return;
            }
        }
        notify(guard.status());
    }

    /// 연결 시도(Connecting) 또는 재연결 대기(Reconnecting) 중이면 취소하고 true.
    /// 그 밖의 상태면 현재 상태를 유지하고 false.
    /// (UI 상태가 늦어 이미 Registering 등으로 넘어간 뒤 취소가 들어와도 세션을 버리지 않는다)
    pub fn abort_connecting(&self, notify: impl FnOnce(ServerStatus)) -> bool {
        let mut guard = self.runtime.lock().unwrap();
        match std::mem::take(&mut *guard) {
            ServerRuntime::Connecting { handle } | ServerRuntime::Reconnecting { handle } => {
                handle.abort();
                *guard = ServerRuntime::Disconnected;
                notify(guard.status());
                true
            }
            other => {
                *guard = other;
                false
            }
        }
    }

    /// 액터 쪽 전이 공통: runtime이 `owner`의 것이고 `next`가 Ok를 돌려주면 그 상태로 바꾸고
    /// notify한다. 아니면 현재 상태를 그대로 두고 false.
    fn transition_owned(
        &self,
        owner: task::Id,
        next: impl FnOnce(ServerRuntime) -> Result<ServerRuntime, ServerRuntime>,
        notify: impl FnOnce(ServerStatus),
    ) -> bool {
        let mut guard = self.runtime.lock().unwrap();
        if !guard.is_owned_by(owner) {
            return false;
        }
        match next(std::mem::take(&mut *guard)) {
            Ok(runtime) => {
                *guard = runtime;
                notify(guard.status());
                true
            }
            Err(runtime) => {
                *guard = runtime;
                false
            }
        }
    }

    /// 연결 성공: Connecting → Registering. 아니면(예: 취소됨) 현재 상태를 유지하고 tx는 버린다.
    pub fn transition_to_registering(
        &self,
        owner: task::Id,
        tx: UnboundedSender<ServerCommand>,
        notify: impl FnOnce(ServerStatus),
    ) -> bool {
        self.transition_owned(
            owner,
            |runtime| match runtime {
                ServerRuntime::Connecting { handle } => {
                    Ok(ServerRuntime::Registering { tx, handle })
                }
                other => Err(other),
            },
            notify,
        )
    }

    /// 등록 완료: Registering → Connected. 아니면(예: Disconnecting) 현재 상태를 유지한다.
    pub fn transition_to_connected(
        &self,
        owner: task::Id,
        notify: impl FnOnce(ServerStatus),
    ) -> bool {
        self.transition_owned(
            owner,
            |runtime| match runtime {
                ServerRuntime::Registering { tx, handle } => {
                    Ok(ServerRuntime::Connected { tx, handle })
                }
                other => Err(other),
            },
            notify,
        )
    }

    /// 세션이 끊겨 재연결 대기로: Connecting/Registering/Connected → Reconnecting (tx는 버린다).
    /// 그 밖의 상태(사용자가 이미 끊었거나 취소함)면 현재 상태를 유지하고 false.
    pub fn transition_to_reconnecting(
        &self,
        owner: task::Id,
        notify: impl FnOnce(ServerStatus),
    ) -> bool {
        self.transition_owned(
            owner,
            |runtime| match runtime {
                ServerRuntime::Connecting { handle }
                | ServerRuntime::Registering { handle, .. }
                | ServerRuntime::Connected { handle, .. } => {
                    Ok(ServerRuntime::Reconnecting { handle })
                }
                other => Err(other),
            },
            notify,
        )
    }

    /// 재연결 대기가 끝나 다시 접속: Reconnecting → Connecting. 그 밖의 상태면 유지하고 false.
    pub fn transition_to_connecting(
        &self,
        owner: task::Id,
        notify: impl FnOnce(ServerStatus),
    ) -> bool {
        self.transition_owned(
            owner,
            |runtime| match runtime {
                ServerRuntime::Reconnecting { handle } => Ok(ServerRuntime::Connecting { handle }),
                other => Err(other),
            },
            notify,
        )
    }

    /// 액터 종료(사용자 끊기 완료): → Disconnected.
    pub fn transition_to_disconnected(
        &self,
        owner: task::Id,
        notify: impl FnOnce(ServerStatus),
    ) -> bool {
        self.transition_owned(owner, |_| Ok(ServerRuntime::Disconnected), notify)
    }

    /// 재시도하지 않는 실패로 액터 종료: → Failed.
    pub fn transition_to_failed(
        &self,
        owner: task::Id,
        error: String,
        notify: impl FnOnce(ServerStatus),
    ) -> bool {
        self.transition_owned(owner, |_| Ok(ServerRuntime::Failed { error }), notify)
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

    /// 아무것도 하지 않는 액터로 연결을 시작하고(Connecting) 그 task id를 돌려준다.
    /// 테스트 본문이 액터 대신 이 id로 액터 쪽 전이를 호출한다.
    fn start_idle(server: &ServerState) -> task::Id {
        let mut id = None;
        assert!(server.start_connecting(
            || {
                let handle = tokio::spawn(std::future::pending::<()>());
                id = Some(handle.id());
                handle
            },
            |_| {}
        ));
        id.unwrap()
    }

    /// notify로 받은 상태들을 모은다
    #[derive(Default)]
    struct Recorder(std::sync::Arc<Mutex<Vec<ServerStatus>>>);

    impl Recorder {
        fn notify(&self) -> impl FnOnce(ServerStatus) + use<> {
            let log = self.0.clone();
            move |status| log.lock().unwrap().push(status)
        }

        fn seen(&self) -> Vec<ServerStatus> {
            self.0.lock().unwrap().clone()
        }
    }

    /// 다른 task id
    fn stranger() -> task::Id {
        tokio::spawn(async {}).id()
    }

    #[tokio::test]
    async fn start_connecting_only_from_idle_states() {
        let server = test_server();

        let actor = start_idle(&server);
        assert_eq!(server.status(), ServerStatus::Connecting);

        // 활성 상태에서는 spawn 자체가 일어나지 않는다
        let mut spawned = false;
        assert!(!server.start_connecting(
            || {
                spawned = true;
                tokio::spawn(async {})
            },
            |_| {}
        ));
        assert!(!spawned);

        // Failed에서는 재시작 가능
        assert!(server.transition_to_failed(actor, "refused".to_string(), |_| {}));
        start_idle(&server);
        assert_eq!(server.status(), ServerStatus::Connecting);
    }

    #[tokio::test]
    async fn start_connecting_refused_while_disconnecting() {
        let server = test_server();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let actor = start_idle(&server);
        assert!(server.transition_to_registering(actor, tx, |_| {}));
        server.disconnect(|_| {});
        assert_eq!(server.status(), ServerStatus::Disconnecting);

        // 이전 액터가 아직 끝나지 않았으므로 새 액터를 띄우면 안 된다
        // (띄우면 이전 액터의 종료 전이가 새 세션 상태를 덮어쓴다)
        assert!(!server.start_connecting(|| tokio::spawn(async {}), |_| {}));
        assert_eq!(server.status(), ServerStatus::Disconnecting);
    }

    #[tokio::test]
    async fn mismatched_transitions_keep_current_state() {
        let server = test_server();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        // Disconnected에서 registering/connected 전이는 no-op
        assert!(!server.transition_to_registering(stranger(), tx.clone(), |_| {}));
        assert_eq!(server.status(), ServerStatus::Disconnected);
        assert!(!server.transition_to_connected(stranger(), |_| {}));
        assert_eq!(server.status(), ServerStatus::Disconnected);

        // Disconnecting 중 늦게 도착한 RPL_WELCOME은 Disconnecting을 덮어쓰지 않는다
        // (이전 구현은 Disconnected로 바꾸며 handle을 유실했다)
        let actor = start_idle(&server);
        assert!(server.transition_to_registering(actor, tx, |_| {}));
        server.disconnect(|_| {});
        assert!(!server.transition_to_connected(actor, |_| {}));
        assert_eq!(server.status(), ServerStatus::Disconnecting);
    }

    /// Connecting이 아닐 때의 취소는 현재 상태(예: Registering 세션)를 버리지 않는다.
    /// (이전: 꺼낸 상태를 버려 Disconnected로 남기고 tx/handle을 잃었다)
    #[tokio::test]
    async fn abort_connecting_keeps_other_states() {
        let server = test_server();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let actor = start_idle(&server);
        assert!(server.transition_to_registering(actor, tx, |_| {}));

        assert!(!server.abort_connecting(|_| {}));
        assert_eq!(server.status(), ServerStatus::Registering);

        // Disconnected/Failed도 그대로
        assert!(server.transition_to_failed(actor, "x".to_string(), |_| {}));
        assert!(!server.abort_connecting(|_| {}));
        assert_eq!(server.status(), ServerStatus::Failed);
    }

    #[tokio::test]
    async fn reconnecting_transitions_keep_actor_handle() {
        let server = test_server();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        // Connected → Reconnecting: 커맨드 채널은 버린다
        let actor = start_idle(&server);
        assert!(server.transition_to_registering(actor, tx, |_| {}));
        assert!(server.transition_to_connected(actor, |_| {}));
        assert!(server.transition_to_reconnecting(actor, |_| {}));
        assert_eq!(server.status(), ServerStatus::Reconnecting);
        assert!(server.is_active());
        assert!(server.send_command(ServerCommand::Quit).is_err());

        // Reconnecting → Connecting → (접속 실패) Reconnecting
        assert!(server.transition_to_connecting(actor, |_| {}));
        assert_eq!(server.status(), ServerStatus::Connecting);
        assert!(server.transition_to_reconnecting(actor, |_| {}));
        assert_eq!(server.status(), ServerStatus::Reconnecting);

        // 액터가 끝나면(Disconnected) 그 뒤의 전이는 모두 no-op
        assert!(server.transition_to_disconnected(actor, |_| {}));
        assert!(!server.transition_to_connecting(actor, |_| {}));
        assert!(!server.transition_to_reconnecting(actor, |_| {}));
        assert_eq!(server.status(), ServerStatus::Disconnected);
    }

    /// 재연결 대기 중 취소/끊기는 대기 중인 액터를 멈추고 Disconnected로 만든다.
    #[tokio::test]
    async fn cancel_and_disconnect_stop_reconnecting() {
        let server = test_server();

        let actor = start_idle(&server);
        assert!(server.transition_to_reconnecting(actor, |_| {}));
        assert!(server.abort_connecting(|_| {}));
        assert_eq!(server.status(), ServerStatus::Disconnected);

        let actor = start_idle(&server);
        assert!(server.transition_to_reconnecting(actor, |_| {}));
        server.disconnect(|_| {});
        assert_eq!(server.status(), ServerStatus::Disconnected);
    }

    /// 재연결 대기 중 '지금 연결'은 대기 중인 액터를 abort하고 새 액터를 띄운다.
    #[tokio::test]
    async fn start_connecting_replaces_reconnecting_actor() {
        let server = test_server();
        let (old_tx, old_rx) = tokio::sync::oneshot::channel::<()>();
        let mut old = None;
        assert!(server.start_connecting(
            || {
                let handle = tokio::spawn(async move {
                    let _keep = old_tx;
                    std::future::pending::<()>().await
                });
                old = Some(handle.id());
                handle
            },
            |_| {}
        ));
        assert!(server.transition_to_reconnecting(old.unwrap(), |_| {}));

        start_idle(&server);
        assert_eq!(server.status(), ServerStatus::Connecting);
        // 이전 액터가 abort돼 sender가 drop된다
        assert!(
            timeout(Duration::from_secs(5), old_rx)
                .await
                .unwrap()
                .is_err()
        );
    }

    /// 다른 task(이미 교체·취소된 이전 액터 등)의 전이는 반영하지 않고 알리지도 않는다.
    #[tokio::test]
    async fn transitions_from_other_task_are_ignored() {
        let server = test_server();
        let rec = Recorder::default();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        start_idle(&server);
        let other = stranger();

        assert!(!server.transition_to_registering(other, tx, rec.notify()));
        assert!(!server.transition_to_reconnecting(other, rec.notify()));
        assert!(!server.transition_to_failed(other, "x".to_string(), rec.notify()));
        assert!(!server.transition_to_disconnected(other, rec.notify()));
        assert_eq!(server.status(), ServerStatus::Connecting);
        assert!(rec.seen().is_empty());
    }

    /// 사용자가 첫 접속을 취소한 직후 이전 액터가 접속 실패를 기록해도 Disconnected가 유지된다.
    #[tokio::test]
    async fn failure_after_cancel_is_ignored() {
        let server = test_server();
        let rec = Recorder::default();
        let actor = start_idle(&server);

        assert!(server.abort_connecting(rec.notify()));
        assert!(!server.transition_to_failed(actor, "refused".to_string(), rec.notify()));
        assert_eq!(server.status(), ServerStatus::Disconnected);
        assert_eq!(rec.seen(), [ServerStatus::Disconnected]);
    }

    /// '지금 연결'로 교체된 이전 액터는 새 연결 상태를 덮어쓰거나 늦은 이벤트를 보내지 못한다.
    #[tokio::test]
    async fn replaced_actor_cannot_touch_new_connection() {
        let server = test_server();
        let rec = Recorder::default();
        let old = start_idle(&server);
        assert!(server.transition_to_reconnecting(old, rec.notify()));

        let new = start_idle(&server);
        assert!(!server.transition_to_connecting(old, rec.notify()));
        assert!(!server.transition_to_reconnecting(old, rec.notify()));
        assert!(!server.transition_to_failed(old, "x".to_string(), rec.notify()));
        assert_eq!(server.status(), ServerStatus::Connecting);

        // 새 액터의 전이는 그대로 반영된다
        assert!(server.transition_to_reconnecting(new, rec.notify()));
        assert_eq!(
            rec.seen(),
            [ServerStatus::Reconnecting, ServerStatus::Reconnecting]
        );
    }

    /// notify는 바뀐 상태로, 상태가 실제로 바뀐 경우에만 호출된다.
    #[tokio::test]
    async fn notify_only_on_change() {
        let server = test_server();
        let rec = Recorder::default();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        assert!(server.start_connecting(|| tokio::spawn(std::future::pending()), rec.notify()));
        server.disconnect(rec.notify()); // Connecting → Disconnected
        server.disconnect(rec.notify()); // no-op
        assert!(!server.abort_connecting(rec.notify())); // no-op
        let actor = start_idle(&server);
        assert!(server.transition_to_registering(actor, tx, rec.notify()));
        assert!(!server.transition_to_registering(
            actor,
            tokio::sync::mpsc::unbounded_channel().0,
            rec.notify()
        ));
        server.disconnect(rec.notify()); // Registering → Disconnecting
        assert!(server.transition_to_disconnected(actor, rec.notify()));

        assert_eq!(
            rec.seen(),
            [
                ServerStatus::Connecting,
                ServerStatus::Disconnected,
                ServerStatus::Registering,
                ServerStatus::Disconnecting,
                ServerStatus::Disconnected,
            ]
        );
    }

    /// 액터가 락 대기 없이 곧바로 실패를 기록하려 해도 Connecting 확정 이후에 반영된다.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn immediate_actor_failure_ends_in_failed() {
        let server = std::sync::Arc::new(test_server());
        let actor_server = server.clone();

        assert!(server.start_connecting(
            move || {
                tokio::spawn(async move {
                    actor_server.transition_to_failed(
                        tokio::task::id(),
                        "refused".to_string(),
                        |_| {},
                    );
                })
            },
            |_| {}
        ));

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
