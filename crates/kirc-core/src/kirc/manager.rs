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

    fn is_shutting_down(&self) -> bool {
        if let Some(state) = self.app_state() {
            state == AppState::ShuttingDown
        } else {
            false
        }
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

        info!(server_id = %server_id, "Spawn server actor");
        let actor = ServerActor::new(server_id, server.clone(), self.ui_sink.clone());
        let handle = tokio::spawn(actor.run());

        server.transition_to_connecting(handle);
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
            if let Some(server) = self.kirc_state.get_server(sid) {
                if server.is_active() {
                    return Err(anyhow!("Already connecting or connected"));
                }
                sid
            } else {
                return Err(anyhow!("Server not found"));
            }
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
            if server.abort_connecting() {
                emit_server_status(&self.ui_sink, server_id, ServerStatus::Failed);
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
        server.insert_channel(channel_name, false);

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
        server.remove_channel(channel_id);

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
