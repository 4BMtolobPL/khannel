use crate::error::MyCustomError;
use crate::kirc::event::{emit_channel_lock_changed, EventSender};
use crate::kirc::manager::KircManager;
use crate::kirc::types::{ServerCommand, ServerId, ServerStatus};
use anyhow::Context;
use std::sync::Arc;

pub use crate::kirc::commands::payload::{
    ChangeNickPayload, ChannelInfo, ChannelPayload, ConnectServerPayload, ServerInfo,
};
use tracing::{info, instrument};

/// UI 프레임워크를 통한 명령 진입점 관련 facade.
///
/// Tauri 시절 `#[tauri::command]` 데코레이터가 물고 있던 역할을 대신한다.
/// GPUI 액션/테스트에서 이 메서드들을 직접 호출한다.
pub struct KircApi {
    manager: Arc<KircManager>,
}

impl KircApi {
    pub fn new(manager: Arc<KircManager>) -> Self {
        Self { manager }
    }

    pub fn manager(&self) -> &Arc<KircManager> {
        &self.manager
    }

    pub fn init_servers(&self) {
        self.manager.process_auto_connect();
    }

    pub fn get_servers(&self) -> Result<Vec<ServerInfo>, MyCustomError> {
        let state = self.manager.kirc_state();
        let servers = state.get_all_servers();
        let mut infos = Vec::new();

        for (id, server_state) in servers {
            let config = server_state.config();
            let channel_infos = server_state
                .channels()
                .into_iter()
                .map(|(name, s)| ChannelInfo::new(&name, s.locked))
                .collect();

            let server_info = ServerInfo::builder()
                .id(id)
                .name(config.server())
                .host(config.server())
                .port(config.port())
                .tls(config.use_tls())
                .nickname(config.nickname())
                .status(server_state.status())
                .channels(channel_infos)
                .build();

            infos.push(server_info);
        }

        Ok(infos)
    }

    pub fn connect_server(&self, payload: ConnectServerPayload) -> Result<(), MyCustomError> {
        self.manager
            .connect_server(payload.server_id(), payload.to_config())
            .map_err(MyCustomError::Anyhow)
    }

    pub fn join_channel(&self, server_id: ServerId, channel: &str) -> Result<(), MyCustomError> {
        info!("Command: join channel invoked, server_id: {server_id}, channel: {channel}");
        self.manager
            .join_channel(server_id, channel)
            .map_err(MyCustomError::Anyhow)
    }

    pub fn leave_channel(&self, payload: &ChannelPayload) -> Result<(), MyCustomError> {
        info!("Command: part channel invoked");
        self.manager
            .part_channel(payload.server_id(), payload.channel())
            .map_err(MyCustomError::Anyhow)
    }

    pub fn send_message(
        &self,
        server_id: ServerId,
        target: &str,
        message: &str,
    ) -> Result<(), MyCustomError> {
        info!("Command: send message invoked, server_id: {server_id}, target: {target}");
        let state = self.manager.kirc_state();

        // 1. 정책 체크
        if state.is_channel_locked(server_id, target) {
            return Err(MyCustomError::Anyhow(anyhow::anyhow!("Channel is locked")));
        }

        // 2. 서버 runtime 접근
        let server = state.get_server(server_id).context("Can't find server")?;
        server.send_command(ServerCommand::Privmsg {
            target: target.to_string(),
            message: message.to_string(),
        })?;

        Ok(())
    }

    pub fn cancel_connect(&self, server_id: ServerId) -> Result<(), MyCustomError> {
        info!("Command: cancel connect invoked, server_id: {server_id}");

        self.manager
            .cancel_connect(server_id)
            .map_err(MyCustomError::Anyhow)
    }

    pub fn disconnect_server(&self, server_id: ServerId) -> Result<(), MyCustomError> {
        self.manager
            .disconnect_server(server_id)
            .map_err(MyCustomError::Anyhow)
    }

    pub fn lock_channel(
        &self,
        payload: &ChannelPayload,
        ui_sink: &EventSender,
    ) -> Result<(), MyCustomError> {
        let state = self.manager.kirc_state();
        let server = state
            .get_server(payload.server_id())
            .context("Can't find server")?;
        server.set_channel_locked(payload.channel(), true);
        state.persist();

        emit_channel_lock_changed(ui_sink, payload.server_id(), payload.channel(), true);

        Ok(())
    }

    pub fn unlock_channel(
        &self,
        payload: &ChannelPayload,
        ui_sink: &EventSender,
    ) -> Result<(), MyCustomError> {
        let state = self.manager.kirc_state();
        if let Some(server) = state.get_server(payload.server_id()) {
            server.set_channel_locked(payload.channel(), false);
            state.persist();
        }

        emit_channel_lock_changed(ui_sink, payload.server_id(), payload.channel(), false);

        Ok(())
    }

    pub fn is_channel_locked(&self, payload: &ChannelPayload) -> Result<bool, MyCustomError> {
        let state = self.manager.kirc_state();
        Ok(state.is_channel_locked(payload.server_id(), payload.channel()))
    }

    #[instrument(skip(self), fields(server_id = %payload.server_id))]
    pub fn change_nickname(&self, payload: &ChangeNickPayload) -> Result<(), MyCustomError> {
        info!(event="change_nickname", new_nick=%payload.new_nick, "Command: change nickname invoked");

        let state = self.manager.kirc_state();
        let server = state
            .get_server(payload.server_id)
            .context("Can't find server")?;

        // 연결되어 있지 않으면(접속 실패 — 닉 선점 등 — 포함) 서버에 보낼 수 없으므로
        // 접속 닉을 직접 바꾼다 (필요하면 다시 연결)
        if !matches!(
            server.status(),
            ServerStatus::Connected | ServerStatus::Registering
        ) {
            return self
                .manager
                .change_nickname_offline(payload.server_id, &payload.new_nick)
                .map_err(MyCustomError::Anyhow);
        }

        server.send_command(ServerCommand::Nick(payload.new_nick.clone()))?;
        // 서버가 이 닉으로 바꿔 주면(NICK 이벤트) 접속 닉으로 저장한다 — core::handle_message
        server.request_nick_change(&payload.new_nick);

        // 닉네임 변경시 core::handle_message에서 자동으로 ui_event가 emit 됨

        Ok(())
    }
}

pub mod payload {
    use crate::kirc::types::server::ServerConfig;
    use crate::kirc::types::{ChannelId, ServerId, ServerStatus};
    use serde::{Deserialize, Serialize};

    #[derive(Deserialize, Debug, Clone)]
    pub struct ConnectServerPayload {
        server_id: Option<ServerId>,
        host: String,
        port: u16,
        tls: bool,
        nickname: String,
    }

    impl ConnectServerPayload {
        pub fn new(
            server_id: Option<ServerId>,
            host: String,
            port: u16,
            tls: bool,
            nickname: String,
        ) -> Self {
            Self {
                server_id,
                host,
                port,
                tls,
                nickname,
            }
        }

        pub fn server_id(&self) -> Option<ServerId> {
            self.server_id
        }

        pub fn to_config(&self) -> ServerConfig {
            ServerConfig::new(
                self.host.clone(),
                self.port,
                self.tls,
                self.nickname.clone(),
            )
        }
    }

    #[derive(Deserialize, Debug, Clone)]
    #[serde(rename_all = "camelCase")]
    pub struct ChannelPayload {
        server_id: ServerId,
        channel: ChannelId,
    }

    impl ChannelPayload {
        pub fn new(server_id: ServerId, channel: ChannelId) -> Self {
            Self { server_id, channel }
        }

        pub fn server_id(&self) -> ServerId {
            self.server_id
        }

        pub fn channel(&self) -> &str {
            &self.channel
        }
    }

    #[derive(Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ChannelInfo {
        name: String,
        locked: bool,
    }

    impl ChannelInfo {
        pub fn new(name: &str, locked: bool) -> Self {
            Self {
                name: name.to_string(),
                locked,
            }
        }
    }

    #[derive(Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ServerInfo {
        pub id: ServerId,
        pub name: String,
        pub host: String,
        pub port: u16,
        pub tls: bool,
        pub nickname: String,
        pub status: ServerStatus,
        pub channels: Vec<ChannelInfo>,
    }

    impl ServerInfo {
        pub fn builder() -> ServerInfoBuilder {
            ServerInfoBuilder::new()
        }
    }

    impl ChannelInfo {
        pub fn name(&self) -> &str {
            &self.name
        }

        pub fn locked(&self) -> bool {
            self.locked
        }
    }

    #[derive(Default)]
    pub struct ServerInfoBuilder {
        id: Option<ServerId>,
        name: Option<String>,
        host: Option<String>,
        port: Option<u16>,
        tls: Option<bool>,
        nickname: Option<String>,
        status: Option<ServerStatus>,
        channels: Option<Vec<ChannelInfo>>,
    }

    impl ServerInfoBuilder {
        fn new() -> Self {
            Self::default()
        }

        pub fn build(&self) -> ServerInfo {
            if self.id.is_none()
                || self.name.is_none()
                || self.host.is_none()
                || self.port.is_none()
                || self.tls.is_none()
                || self.nickname.is_none()
                || self.status.is_none()
                || self.channels.is_none()
            {
                panic!("ServerInfoBuilder: build() called with incomplete data");
            }

            ServerInfo {
                id: self.id.unwrap(),
                name: self.name.clone().unwrap(),
                host: self.host.clone().unwrap(),
                port: self.port.unwrap(),
                tls: self.tls.unwrap(),
                nickname: self.nickname.clone().unwrap(),
                status: self.status.clone().unwrap(),
                channels: self.channels.clone().unwrap(),
            }
        }

        pub fn id(&mut self, id: ServerId) -> &mut Self {
            self.id = Some(id);
            self
        }

        pub fn name(&mut self, name: &str) -> &mut Self {
            self.name = Some(name.to_string());
            self
        }

        pub fn host(&mut self, host: &str) -> &mut Self {
            self.host = Some(host.to_string());
            self
        }

        pub fn port(&mut self, port: u16) -> &mut Self {
            self.port = Some(port);
            self
        }

        pub fn tls(&mut self, tls: bool) -> &mut Self {
            self.tls = Some(tls);
            self
        }

        pub fn nickname(&mut self, nickname: &str) -> &mut Self {
            self.nickname = Some(nickname.to_string());
            self
        }

        pub fn status(&mut self, status: ServerStatus) -> &mut Self {
            self.status = Some(status);
            self
        }

        pub fn channels(&mut self, channels: Vec<ChannelInfo>) -> &mut Self {
            self.channels = Some(channels);
            self
        }
    }

    #[derive(Deserialize, Debug, Clone)]
    #[serde(rename_all = "camelCase")]
    pub struct ChangeNickPayload {
        pub server_id: ServerId,
        pub new_nick: String,
    }

    impl ChangeNickPayload {
        pub fn new(server_id: ServerId, new_nick: String) -> Self {
            Self {
                server_id,
                new_nick,
            }
        }
    }
}
