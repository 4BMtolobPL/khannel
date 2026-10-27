use crate::error::MyCustomError;
use crate::kirc::commands::payload::{
    ChangeNickPayload, ChannelInfo, ChannelPayload, ConnectServerPayload, ServerInfo,
};
use crate::kirc::manager::KircManager;
use crate::kirc::state::kirc::KircState;
use crate::kirc::types::{ServerCommand, ServerId};
use anyhow::Context;
use std::sync::Arc;
use tauri::{AppHandle, State};
use tracing::{info, instrument};

#[tauri::command]
pub(crate) async fn init_servers(manager: State<'_, KircManager>) -> Result<(), MyCustomError> {
    manager.process_auto_connect();
    Ok(())
}

#[tauri::command]
pub(crate) fn get_servers(
    state: State<'_, Arc<KircState>>,
) -> Result<Vec<ServerInfo>, MyCustomError> {
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

#[tauri::command]
pub(crate) async fn connect_server(
    payload: ConnectServerPayload,
    manager: State<'_, KircManager>,
) -> Result<(), MyCustomError> {
    manager
        .connect_server(payload.server_id(), payload.to_config())
        .map_err(MyCustomError::Anyhow)?;

    Ok(())
}

#[tauri::command]
pub(crate) fn join_channel(
    server_id: ServerId,
    channel: String,
    manager: State<KircManager>,
) -> Result<(), MyCustomError> {
    info!("Tauri command: join channel invoked, server_id: {server_id}, channel: {channel}");
    manager
        .join_channel(server_id, &channel)
        .map_err(MyCustomError::Anyhow)?;

    Ok(())
}

#[tauri::command]
pub(crate) fn leave_channel(
    payload: ChannelPayload,
    manager: State<KircManager>,
) -> Result<(), MyCustomError> {
    info!("Tauri command: part channel invoked");
    manager
        .part_channel(payload.server_id(), payload.channel())
        .map_err(MyCustomError::Anyhow)?;

    Ok(())
}

#[tauri::command]
pub(crate) fn send_message(
    server_id: ServerId,
    target: String,
    message: String,
    state: State<Arc<KircState>>,
) -> Result<(), MyCustomError> {
    info!("Tauri command: send message invoked, server_id: {server_id}, target: {target}, message: {message}");

    // 1. 정책 체크
    if state.is_channel_locked(server_id, &target) {
        return Err(MyCustomError::Anyhow(anyhow::anyhow!("Channel is locked")));
    }

    // 2. 서버 runtime 접근
    let server = state.get_server(server_id).context("Can't find server")?;
    server.send_command(ServerCommand::Privmsg { target, message })?;

    Ok(())
}

#[tauri::command]
pub(crate) fn cancel_connect(
    server_id: ServerId,
    manager: State<'_, KircManager>,
) -> Result<(), MyCustomError> {
    info!("Tauri command: cancel connect invoked, server_id: {server_id}");

    manager
        .cancel_connect(server_id)
        .map_err(MyCustomError::Anyhow)?;

    Ok(())
}

#[tauri::command]
pub(crate) fn disconnect_server(
    server_id: ServerId,
    manager: State<'_, KircManager>,
) -> Result<(), MyCustomError> {
    manager
        .disconnect_server(server_id)
        .map_err(MyCustomError::Anyhow)?;
    Ok(())
}

#[tauri::command]
pub(crate) fn lock_channel(
    payload: ChannelPayload,
    state: State<'_, Arc<KircState>>,
    app_handle: AppHandle,
) -> Result<(), MyCustomError> {
    let server = state
        .get_server(payload.server_id())
        .context("Can't find server")?;
    server.set_channel_locked(payload.channel(), true);

    crate::kirc::emits::emit_channel_lock_changed(
        &app_handle,
        payload.server_id(),
        payload.channel(),
        true,
    )?;

    Ok(())
}

#[tauri::command]
pub(crate) fn unlock_channel(
    payload: ChannelPayload,
    state: State<'_, Arc<KircState>>,
    app_handle: AppHandle,
) -> Result<(), MyCustomError> {
    if let Some(server) = state.get_server(payload.server_id()) {
        server.set_channel_locked(payload.channel(), false);
    }

    crate::kirc::emits::emit_channel_lock_changed(
        &app_handle,
        payload.server_id(),
        payload.channel(),
        false,
    )?;

    Ok(())
}

#[tauri::command]
pub(crate) fn is_channel_locked(
    payload: ChannelPayload,
    state: State<'_, Arc<KircState>>,
) -> Result<bool, MyCustomError> {
    Ok(state.is_channel_locked(payload.server_id(), payload.channel()))
}

#[tauri::command]
#[instrument(skip(state), fields(server_id = %payload.server_id))]
pub(crate) fn change_nickname(
    payload: ChangeNickPayload,
    state: State<Arc<KircState>>,
) -> Result<(), MyCustomError> {
    info!(event="change_nickname", new_nick=%payload.new_nick, "Tauri command: change nickname invoked");

    let server = state
        .get_server(payload.server_id)
        .context("Can't find server")?;
    server.send_command(ServerCommand::Nick(payload.new_nick))?;

    // 닉네임 변경시 core::handle_message에서 자동으로 ui_event가 emit 됨

    Ok(())
}

mod payload {
    use crate::kirc::types::server::ServerConfig;
    use crate::kirc::types::{ChannelId, ServerId, ServerStatus};
    use serde::{Deserialize, Serialize};

    #[derive(Deserialize, Debug, Clone)]
    pub(crate) struct ConnectServerPayload {
        server_id: Option<ServerId>,
        host: String,
        port: u16,
        tls: bool,
        nickname: String,
    }

    impl ConnectServerPayload {
        pub(super) fn server_id(&self) -> Option<ServerId> {
            self.server_id
        }

        pub(super) fn to_config(&self) -> ServerConfig {
            ServerConfig::new(
                self.host.to_string(),
                self.port,
                self.tls,
                self.nickname.to_string(),
            )
        }
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub(crate) struct ChannelPayload {
        server_id: ServerId,
        channel: ChannelId,
    }

    impl ChannelPayload {
        pub(super) fn server_id(&self) -> ServerId {
            self.server_id
        }

        pub(super) fn channel(&self) -> &str {
            &self.channel
        }
    }

    #[derive(Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    pub(crate) struct ChannelInfo {
        name: String,
        locked: bool,
    }

    impl ChannelInfo {
        pub(super) fn new(name: &str, locked: bool) -> Self {
            Self {
                name: name.to_string(),
                locked,
            }
        }
    }

    #[derive(Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    pub(crate) struct ServerInfo {
        id: ServerId,
        name: String,
        host: String,
        port: u16,
        tls: bool,
        nickname: String,
        status: ServerStatus,
        channels: Vec<ChannelInfo>,
    }

    impl ServerInfo {
        pub(super) fn builder() -> ServerInfoBuilder {
            ServerInfoBuilder::new()
        }
    }

    #[derive(Default)]
    pub(super) struct ServerInfoBuilder {
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

        pub(super) fn build(&self) -> ServerInfo {
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

        pub(super) fn id(&mut self, id: ServerId) -> &mut Self {
            self.id = Some(id);
            self
        }

        pub(super) fn name(&mut self, name: &str) -> &mut Self {
            self.name = Some(name.to_string());
            self
        }

        pub(super) fn host(&mut self, host: &str) -> &mut Self {
            self.host = Some(host.to_string());
            self
        }

        pub(super) fn port(&mut self, port: u16) -> &mut Self {
            self.port = Some(port);
            self
        }

        pub(super) fn tls(&mut self, tls: bool) -> &mut Self {
            self.tls = Some(tls);
            self
        }

        pub(super) fn nickname(&mut self, nickname: &str) -> &mut Self {
            self.nickname = Some(nickname.to_string());
            self
        }

        pub(super) fn status(&mut self, status: ServerStatus) -> &mut Self {
            self.status = Some(status);
            self
        }

        pub(super) fn channels(&mut self, channels: Vec<ChannelInfo>) -> &mut Self {
            self.channels = Some(channels);
            self
        }
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    #[derive(Debug)]
    pub(crate) struct ChangeNickPayload {
        pub(super) server_id: ServerId,
        pub(super) new_nick: String,
    }
}
