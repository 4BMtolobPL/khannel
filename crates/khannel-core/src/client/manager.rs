use crate::client::core::ServerActor;
use crate::client::event::EventSender;
use crate::client::event::{
    emit_server_added, emit_server_removed, emit_server_status, emit_system_message, emit_ui_event,
};
use crate::client::state::app::AppState;
use crate::client::state::khannel::KhannelState;
use crate::client::types::server::ServerConfig;
use crate::client::types::{ServerCommand, ServerId, ServerStatus};
use anyhow::{Context, anyhow};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use tracing::{debug, info};

pub struct KhannelManager {
    khannel_state: Arc<KhannelState>,
    app_state: AtomicU8,
    ui_sink: EventSender,
}

// private
impl KhannelManager {
    fn app_state(&self) -> Option<AppState> {
        AppState::from_u8(self.app_state.load(Ordering::Acquire))
    }

    fn set_app_state(&self, state: AppState) {
        self.app_state.store(state.as_u8(), Ordering::Release);
    }

    /// 종료 중이거나 종료가 끝났으면 true — 이후의 연결/서버 추가 명령을 거부한다.
    fn is_shutting_down(&self) -> bool {
        matches!(
            self.app_state(),
            Some(AppState::ShuttingDown | AppState::Terminated)
        )
    }

    fn complete_shutdown(&self) {
        self.set_app_state(AppState::Terminated);
    }

    fn prepare_shutdown(&self) {
        let _ = self.khannel_state.save_snapshot();
        self.set_app_state(AppState::ShuttingDown);
    }

    pub fn khannel_state(&self) -> &Arc<KhannelState> {
        &self.khannel_state
    }

    fn run_server(&self, server_id: ServerId) -> anyhow::Result<()> {
        let server = self
            .khannel_state
            .get_server(server_id)
            .context("Server not found")?;

        let actor = ServerActor::new(
            server_id,
            server.clone(),
            self.ui_sink.clone(),
            Arc::downgrade(&self.khannel_state),
        );
        // spawn과 Connecting 전이를 원자적으로 수행 (ServerState::start_connecting 참고)
        if !server.start_connecting(|| {
            info!(server_id = %server_id, "Spawn server actor");
            tokio::spawn(actor.run())
        }) {
            return Err(anyhow!("Already connecting, connected or disconnecting"));
        }
        Ok(())
    }
}

// pub
impl KhannelManager {
    pub fn connect_server(
        &self,
        server_id: Option<ServerId>,
        config: ServerConfig,
    ) -> anyhow::Result<()> {
        if self.is_shutting_down() {
            return Err(anyhow!("Application is shutting down"));
        }

        let server_id = if let Some(sid) = server_id {
            // 이미 활성/Disconnecting 상태인지는 run_server(start_connecting)가 원자적으로 판정
            if self.khannel_state.get_server(sid).is_none() {
                return Err(anyhow!("Server not found"));
            }
            sid
        } else {
            // New server
            let sid = self.khannel_state.add_server(config.clone())?;
            emit_server_added(
                &self.ui_sink,
                sid,
                config.server(),
                config.port(),
                config.use_tls(),
                config.nickname(),
                ServerStatus::Disconnected,
            );
            sid
        };

        self.run_server(server_id)?;
        emit_server_status(&self.ui_sink, server_id, ServerStatus::Connecting);

        Ok(())
    }

    pub fn disconnect_server(&self, server_id: ServerId) -> anyhow::Result<()> {
        if let Some(server) = self.khannel_state.get_server(server_id) {
            server.disconnect();
            emit_server_status(&self.ui_sink, server_id, server.status());
            Ok(())
        } else {
            Err(anyhow!("Server not found"))
        }
    }

    /// 연결되어 있지 않은 서버의 닉 변경: 서버에 NICK을 보낼 수 없으므로 접속 닉을 바로 바꿔
    /// 저장한다. 접속에 실패한 상태(Failed — 예: 닉 선점)였다면 새 닉으로 바로 다시 연결한다.
    /// 사용자가 직접 끊은 상태(Disconnected)는 다시 연결하지 않고 다음 접속부터 적용한다.
    pub fn change_nickname_offline(
        &self,
        server_id: ServerId,
        new_nick: &str,
    ) -> anyhow::Result<()> {
        let server = self
            .khannel_state
            .get_server(server_id)
            .context("Can't find server")?;
        let status = server.status();
        if !matches!(status, ServerStatus::Disconnected | ServerStatus::Failed) {
            return Err(anyhow!(
                "서버에 연결하는 중에는 닉네임을 바꿀 수 없습니다. 잠시 후 다시 시도하세요"
            ));
        }

        let old_nick = server.current_nickname();
        server.set_nickname(new_nick);
        self.khannel_state.persist();
        info!(server_id = %server_id, new_nick = %new_nick, "Nickname changed while offline");

        // UI의 Server.nickname / 닉변 모달 성공 처리는 NICK 이벤트로 맞춘다
        emit_ui_event(&self.ui_sink)
            .nick(server_id, old_nick, new_nick.to_string())
            .emit();

        if status == ServerStatus::Failed {
            emit_system_message(
                &self.ui_sink,
                server_id,
                &format!("닉네임을 '{new_nick}'(으)로 바꿔 다시 연결합니다."),
            );
            self.connect_server(Some(server_id), server.config())?;
        } else {
            emit_system_message(
                &self.ui_sink,
                server_id,
                &format!("닉네임을 '{new_nick}'(으)로 바꿨습니다. 다음 접속부터 적용됩니다."),
            );
        }
        Ok(())
    }

    /// 서버 삭제: 상태에서 빼고 즉시 저장한 뒤(재시작해도 다시 접속하지 않게), 연결 중이었다면
    /// 백그라운드에서 QUIT을 보내 정리하고 UI에 알린다.
    pub fn remove_server(&self, server_id: ServerId) -> anyhow::Result<()> {
        if self.is_shutting_down() {
            return Err(anyhow!("Application is shutting down"));
        }
        let server = self
            .khannel_state
            .remove_server(server_id)
            .context("Server not found")?;
        self.khannel_state.persist();
        info!(server_id = %server_id, "Server removed");

        // 연결 정리는 기다리지 않는다 — 액터가 남은 상태 전이/이벤트를 보내도 이미 맵에서 빠진
        // 서버라 저장/UI에 영향이 없다(UI는 없는 서버의 이벤트를 무시한다).
        let runtime = server.take_runtime();
        tokio::spawn(runtime.graceful_shutdown());

        emit_server_removed(&self.ui_sink, server_id);
        Ok(())
    }

    pub fn cancel_connect(&self, server_id: ServerId) -> anyhow::Result<()> {
        if let Some(server) = self.khannel_state.get_server(server_id) {
            // 사용자 취소이므로 실제 전이 결과(Disconnected)를 그대로 알린다
            // (이전: 상태는 Disconnected인데 UI에는 Failed를 보냈다)
            if server.abort_connecting() {
                emit_server_status(&self.ui_sink, server_id, server.status());
            }
            Ok(())
        } else {
            Err(anyhow!("Server not found"))
        }
    }

    pub fn process_auto_connect(&self) {
        let server_ids: Vec<ServerId> = self
            .khannel_state
            .get_all_servers()
            .keys()
            .cloned()
            .collect();
        debug!(server_id = ?server_ids, "Auto-connecting servers");
        for server_id in server_ids {
            let _ = self.run_server(server_id);
        }
    }

    pub fn join_channel(&self, server_id: ServerId, channel_name: &str) -> anyhow::Result<()> {
        let server = self
            .khannel_state
            .get_server(server_id)
            .context("Can't find server")?;
        server.send_command(ServerCommand::Join(channel_name.to_string()))?;
        // 기존 채널 항목이 있으면 그대로 둔다 — 재조인으로 lock 상태가
        // `locked: false` 덮어쓰기되면 안 된다 (Phase 4 정규화 이후 스냅샷 영속 대상).
        if !server.contains_channel(channel_name) {
            server.insert_channel(channel_name, false);
            self.khannel_state.persist();
        }

        Ok(())
    }

    pub fn part_channel(&self, server_id: ServerId, channel_id: &str) -> anyhow::Result<()> {
        let server = self
            .khannel_state
            .get_server(server_id)
            .context("Can't find server")?;
        server.send_command(ServerCommand::Part {
            channel_name: channel_id.to_string(),
        })?;
        if server.remove_channel(channel_id).is_some() {
            self.khannel_state.persist();
        }

        Ok(())
    }
}

// pub
impl KhannelManager {
    pub fn new(khannel_state: Arc<KhannelState>, ui_sink: EventSender) -> Self {
        Self {
            khannel_state,
            app_state: AtomicU8::new(AppState::Running.as_u8()),
            ui_sink,
        }
    }

    pub async fn shutdown(&self) {
        // 1. 상태 전이 AppState -> ShuttingDown (State method 호출 필요)
        self.prepare_shutdown();

        // 2. 서버 drain 및 runtime 획득
        let runtimes = self.khannel_state.drain_runtimes();

        // 3. 병렬 graceful 종료
        futures::future::join_all(
            runtimes
                .into_iter()
                .map(|runtime| runtime.graceful_shutdown()),
        )
        .await;

        // 4. 최종 상태 AppState -> Terminated
        self.complete_shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::event::UiEvent;
    use crate::client::persistence::ServerStateSnapshot;
    use std::collections::HashMap;
    use std::time::Duration;
    use tokio::sync::mpsc::unbounded_channel;

    /// 연결 즉시 거부되는 포트 (bind 후 바로 닫는다)
    async fn closed_port() -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap().port()
    }

    /// 연결 실패(connection refused) 후 상태가 Failed로 남고 재연결이 반복 가능해야 한다.
    /// (경쟁 자체의 결정적 검증은 state::server 테스트의 immediate_actor_failure_ends_in_failed)
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn fast_connect_failure_is_not_clobbered_by_connecting() {
        let port = closed_port().await;
        let config = ServerConfig::new("127.0.0.1".to_string(), port, false, "nick".to_string());
        let state = Arc::new(KhannelState::from_iter([ServerStateSnapshot::new(
            config.clone(),
            HashMap::new(),
        )]));
        let server_id = *state.get_all_servers().keys().next().unwrap();
        let server = state.get_server(server_id).unwrap();

        let (tx, _rx) = unbounded_channel::<UiEvent>();
        let manager = KhannelManager::new(state.clone(), Arc::new(tx));

        for attempt in 0..200 {
            manager
                .connect_server(Some(server_id), config.clone())
                .unwrap_or_else(|e| panic!("attempt {attempt}: reconnect refused: {e}"));

            // 액터가 실패를 기록할 때까지 대기 (Connecting에 갇히면 타임아웃)
            tokio::time::timeout(Duration::from_secs(5), async {
                while server.status() != ServerStatus::Failed {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            })
            .await
            .unwrap_or_else(|_| panic!("attempt {attempt}: stuck in {:?}", server.status()));
        }
    }
}
