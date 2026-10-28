use crate::kirc::core::ServerActor;
use crate::kirc::event::EventSender;
use crate::kirc::event::{emit_server_added, emit_server_status};
use crate::kirc::state::app::AppState;
use crate::kirc::state::kirc::KircState;
use crate::kirc::types::server::ServerConfig;
use crate::kirc::types::{ServerCommand, ServerId, ServerStatus};
use anyhow::{anyhow, Context};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use tracing::{debug, info};

pub struct KircManager {
    kirc_state: Arc<KircState>,
    app_state: AtomicU8,
    ui_sink: EventSender,
}

// private
impl KircManager {
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
        let _ = self.kirc_state.save_snapshot();
        self.set_app_state(AppState::ShuttingDown);
    }

    pub fn kirc_state(&self) -> &Arc<KircState> {
        &self.kirc_state
    }

    fn run_server(&self, server_id: ServerId) -> anyhow::Result<()> {
        let server = self
            .kirc_state
            .get_server(server_id)
            .context("Server not found")?;

        let actor = ServerActor::new(server_id, server.clone(), self.ui_sink.clone());
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
impl KircManager {
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
            if self.kirc_state.get_server(sid).is_none() {
                return Err(anyhow!("Server not found"));
            }
            sid
        } else {
            // New server
            let sid = self.kirc_state.add_server(config.clone())?;
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
        if let Some(server) = self.kirc_state.get_server(server_id) {
            server.disconnect();
            emit_server_status(&self.ui_sink, server_id, server.status());
            Ok(())
        } else {
            Err(anyhow!("Server not found"))
        }
    }

    pub fn cancel_connect(&self, server_id: ServerId) -> anyhow::Result<()> {
        if let Some(server) = self.kirc_state.get_server(server_id) {
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
        let server_ids: Vec<ServerId> = self.kirc_state.get_all_servers().keys().cloned().collect();
        debug!(server_id = ?server_ids, "Auto-connecting servers");
        for server_id in server_ids {
            let _ = self.run_server(server_id);
        }
    }

    pub fn join_channel(&self, server_id: ServerId, channel_name: &str) -> anyhow::Result<()> {
        let server = self
            .kirc_state
            .get_server(server_id)
            .context("Can't find server")?;
        server.send_command(ServerCommand::Join(channel_name.to_string()))?;
        // 기존 채널 항목이 있으면 그대로 둔다 — 재조인으로 lock 상태가
        // `locked: false` 덮어쓰기되면 안 된다 (Phase 4 정규화 이후 스냅샷 영속 대상).
        if !server.contains_channel(channel_name) {
            server.insert_channel(channel_name, false);
            self.kirc_state.persist();
        }

        Ok(())
    }

    pub fn part_channel(&self, server_id: ServerId, channel_id: &str) -> anyhow::Result<()> {
        let server = self
            .kirc_state
            .get_server(server_id)
            .context("Can't find server")?;
        server.send_command(ServerCommand::Part {
            channel_name: channel_id.to_string(),
        })?;
        if server.remove_channel(channel_id).is_some() {
            self.kirc_state.persist();
        }

        Ok(())
    }
}

// pub
impl KircManager {
    pub fn new(kirc_state: Arc<KircState>, ui_sink: EventSender) -> Self {
        Self {
            kirc_state,
            app_state: AtomicU8::new(AppState::Running.as_u8()),
            ui_sink,
        }
    }

    pub async fn shutdown(&self) {
        // 1. 상태 전이 AppState -> ShuttingDown (State method 호출 필요)
        self.prepare_shutdown();

        // 2. 서버 drain 및 runtime 획득
        let runtimes = self.kirc_state.drain_runtimes();

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
    use crate::kirc::event::UiEvent;
    use crate::kirc::persistence::ServerStateSnapshot;
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
        let state = Arc::new(KircState::from_iter([ServerStateSnapshot::new(
            config.clone(),
            HashMap::new(),
        )]));
        let server_id = *state.get_all_servers().keys().next().unwrap();
        let server = state.get_server(server_id).unwrap();

        let (tx, _rx) = unbounded_channel::<UiEvent>();
        let manager = KircManager::new(state.clone(), Arc::new(tx));

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
