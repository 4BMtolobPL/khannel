use crate::kirc::ctcp::{parse_ctcp, CtcpCommand};
use crate::kirc::emits::{
    emit_change_nick_failed, emit_server_status, emit_system_message, emit_ui_event,
};
use crate::kirc::state::server::ServerState;
use crate::kirc::types::{ServerCommand, ServerId, ServerStatus};
use futures::prelude::*;
use irc::client::prelude::*;
use irc::client::ClientStream;
use irc::error;
use std::sync::Arc;
use tauri::AppHandle;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
use tracing::{debug, error, info, instrument, trace, warn};

/// IRC 서버 하나당 하나의 Actor. 연결 생명주기와 메시지 처리를 소유한다.
pub(in crate::kirc) struct ServerActor {
    server_id: ServerId,
    server: Arc<ServerState>,
    app_handle: AppHandle,
}

impl ServerActor {
    pub(in crate::kirc) fn new(
        server_id: ServerId,
        server: Arc<ServerState>,
        app_handle: AppHandle,
    ) -> Self {
        Self {
            server_id,
            server,
            app_handle,
        }
    }

    /// 액터의 진입점. KircManager가 `tokio::spawn(actor.run())`으로 실행한다.
    #[instrument(name = "server_actor", skip_all, fields(server_id = %self.server_id))]
    pub(in crate::kirc) async fn run(self) {
        debug!(server_id = %self.server_id, "Starting server actor");

        // 1. 연결 수립. 에러는 fail 로 소비
        let (client, stream) = match self.connect().await {
            Ok(connected) => connected,
            Err(e) => {
                self.fail(e.to_string());
                return;
            }
        };

        // 2. 커맨드 채널은 연결 성공 이후에 생성
        let (tx, rx) = unbounded_channel::<ServerCommand>();
        self.server.transition_to_registering(tx);
        let _ = emit_server_status(&self.app_handle, self.server_id, ServerStatus::Registering);

        // 3. 메인 세션: IRC 스트림과 커맨드 채널을 select 로 소비
        trace!(server_id = %self.server_id, "Start server actor loop");
        let outcome = self.run_session(client, stream, rx).await;

        // 4. 세션 종료(정상 종료/연결 단절 공통). 추후 재연결 구현 시 Closed 분기가 재시도 진입점이 된다.
        self.server.transition_to_disconnected();
        let _ = emit_server_status(&self.app_handle, self.server_id, ServerStatus::Disconnected);

        debug!(server_id = %self.server_id, reason = ?outcome, "Server actor session ended");
    }

    fn build_irc_config(&self) -> Config {
        let server_config = self.server.config();

        Config {
            server: Some(server_config.server().to_string()),
            port: Some(server_config.port()),
            use_tls: Some(server_config.use_tls()),
            nickname: Some(server_config.nickname().to_string()),
            // alt 닉네임 직접 제어
            alt_nicks: vec![],
            ..Config::default()
        }
    }

    #[instrument(skip_all, fields(server_id = %self.server_id))]
    async fn connect(&self) -> anyhow::Result<(Client, ClientStream)> {
        let config = self.build_irc_config();

        let mut client = Client::from_config(config).await?;
        client.identify()?;
        let stream = client.stream()?;

        Ok((client, stream))
    }

    /// 메인 세션 루프. 정상 종료(Quit) 또는 연결 단절(Closed)로 종료 사유를 반환한다.
    #[instrument(skip_all, fields(server_id = %self.server_id))]
    async fn run_session(
        &self,
        client: Client,
        mut stream: ClientStream,
        mut rx: UnboundedReceiver<ServerCommand>,
    ) -> SessionOutcome {
        loop {
            tokio::select! {
                // IRC 서버에서 오는 메시지
                result = stream.next() => {
                    match result {
                        Some(Ok(message)) => {
                            let _ = self.handle_message(&client, message);
                        }
                        Some(Err(error::Error::NoUsableNick)) => {
                            // 사용 가능한 닉네임이 없을때 (단순 닉네임 중복 등)
                            // 처음부터 사용가능한 닉네임이 없었다면 identify에서 걸렸을테니 break 하지 않고 continue
                            debug!(event = "irc_stream_error", error = "NoUsableNick", "IRC stream error, No usable nick");

                            // 닉네임 변경은 alt_nick 없이 실패시 emit 후 바로 continue
                            let _ = emit_change_nick_failed(&self.app_handle, self.server_id, "Can't change nickname");

                            continue
                        }
                        Some(Err(e)) => {
                            error!(
                                event = "irc_stream_error",
                                error = ?e,
                                "IRC stream error, connection likely closed"
                            );
                            return SessionOutcome::Closed
                        }
                        None => {
                            warn!(event = "irc_stream_ended", "IRC stream ended unexpectedly");
                            return SessionOutcome::Closed
                        }
                    }
                }
                // 커맨드 채널 명령
                cmd = rx.recv() => {
                    match cmd {
                        Some(ServerCommand::Quit) => {
                            if let Err(e) = client.send_quit("bye") {
                                error!("Failed to send quit message: {e}");
                            }
                            return SessionOutcome::Quit
                        }
                        Some(cmd) => self.handle_server_command(&client, cmd),
                        // 모든 sender가 사라진 경우 (runtime이 정리됨): 수신할 커맨드는 없으므로 세션 종료
                        None => {
                            warn!(event = "command_channel_closed", "All command senders dropped, closing session");
                            return SessionOutcome::Closed
                        }
                    }
                }
            }
        }
    }

    #[instrument(skip_all, fields(server_id = %self.server_id))]
    fn handle_server_command(&self, client: &Client, cmd: ServerCommand) {
        match cmd {
            ServerCommand::Join(ch) => {
                info!(event = "join", channel = %ch);
                if let Err(e) = client.send_join(&ch) {
                    error!("Failed to send join message: {e}");
                }
            }
            ServerCommand::Privmsg { target, message } => {
                if let Err(e) = client.send_privmsg(&target, &message) {
                    error!("Failed to send privmsg: {e}");
                }

                // 자신이 보낸 PRIVMSG를 UI에 반영하기 위해 에코 메시지로 다시 처리
                self.send_echo_message(client, &target, &message);
            }
            ServerCommand::Part { channel_name } => {
                if let Err(e) = client.send_part(&channel_name) {
                    error!("Failed to send part: {e}");
                }
            }
            ServerCommand::Nick(new_nick) => {
                info!(event = "nick", new_nick = %new_nick);
                if let Err(e) = client.send(Command::NICK(new_nick.to_owned())) {
                    error!(event = "nick_send_failed", new_nick = %new_nick, error = %e, "failed to send IRC NICK command");
                }
            }
            ServerCommand::Quit => {
                // Quit은 run_session에서 직접 처리한다 (루프 종료 필요)
                warn!(
                    event = "ignored_command",
                    command = "Quit",
                    "Quit is handled in run_session"
                );
            }
        }
    }

    fn send_echo_message(&self, client: &Client, target: &str, message: &str) {
        let current_nick = self.current_nick();

        match Message::with_tags(None, Some(&current_nick), "PRIVMSG", vec![target, message]) {
            Ok(msg) => {
                if let Err(e) = self.handle_message(client, msg) {
                    error!("Failed to handle echo message: {e}");
                }
            }
            Err(e) => {
                error!("Failed to create echo message: {e}");
            }
        }
    }

    fn current_nick(&self) -> String {
        self.server.current_nickname()
    }

    #[instrument(skip_all, fields(server_id = %self.server_id))]
    fn handle_message(&self, client: &Client, message: Message) -> anyhow::Result<()> {
        let source_nickname = message.source_nickname().unwrap_or("").to_string();

        match message.command {
            Command::PRIVMSG(target, content) => {
                if let Some(ctcp) = parse_ctcp(&content) {
                    info!(target = %target, content = %content, "Received CTCP message");
                    handle_ctcp(client, &source_nickname, ctcp);
                } else {
                    emit_ui_event(&self.app_handle)
                        .user_message(self.server_id, target, source_nickname, content)
                        .emit()?;
                }
            }
            Command::JOIN(chanlist, _chankey, _real_name) => {
                emit_ui_event(&self.app_handle)
                    .join(self.server_id, chanlist, source_nickname)
                    .emit()?;
            }
            Command::PART(chanlist, comment) => {
                emit_ui_event(&self.app_handle)
                    .part(self.server_id, chanlist, source_nickname, comment)
                    .emit()?;
            }
            Command::QUIT(comment) => {
                emit_ui_event(&self.app_handle)
                    .quit(self.server_id, source_nickname, comment)
                    .emit()?;
            }
            Command::NICK(nickname) => {
                // 1. 자기 자신인지 확인
                if self.server.current_nickname() == source_nickname {
                    self.server.set_current_nickname(&nickname);
                }

                // 지금은 백엔드에서 유저목록 관리 x, 나중에 변경될 수 있음
                // 2. 모든 채널에서 유저 닉 변경
                /*for channel in state.channels.values_mut() {
                    if channel.users.remove(&old_nick) {
                        channel.users.insert(new_nick.clone());
                    }
                }*/

                // 3. 프론트로 이벤트 emit
                emit_ui_event(&self.app_handle)
                    .nick(self.server_id, source_nickname, nickname)
                    .emit()?;
            }
            Command::TOPIC(channel, topic) => {
                emit_ui_event(&self.app_handle)
                    .topic(self.server_id, channel, topic)
                    .emit()?;
            }
            Command::ERROR(message) => {
                warn!(message = %message, "Server command error");
                emit_ui_event(&self.app_handle)
                    .error(self.server_id, message)
                    .emit()?;
            }
            Command::Response(Response::RPL_WELCOME, _) => {
                self.server.transition_to_connected();

                // 기존 채널이 존재하면 연결
                for (channel_name, _channel_state) in self.server.channels() {
                    self.server
                        .send_command(ServerCommand::Join(channel_name))?;
                }

                emit_server_status(&self.app_handle, self.server_id, ServerStatus::Connected)?;

                // Optional: Alert system message
                emit_system_message(&self.app_handle, self.server_id, "서버에 연결되었습니다.")?;
            }
            Command::Response(Response::ERR_NICKNAMEINUSE, _) => {
                // 닉네임이 중복된 경우
                // alt nick으로 재시도
                // 그래도 안될경우 오류 던지기
                trace!("닉네임 중복");
            }
            _ => {
                // TODO: Command 다른것도 추가하기
                debug!(event = "unprocessed_irc_message", command = ?message.command);
            }
        }

        Ok(())
    }

    #[instrument(skip_all, fields(server_id = %self.server_id))]
    fn fail(&self, message: String) {
        error!(server_id = %self.server_id, message = %message, "Server actor failed");

        self.server.transition_to_failed(message);
        let _ = emit_server_status(&self.app_handle, self.server_id, ServerStatus::Failed);
    }
}

/// 세션 종료 사유.
///
/// 재연결 구현 시 `Closed` 분기에서 재시도 정책이 결정되는 진입점이 된다.
#[derive(Debug)]
enum SessionOutcome {
    /// ServerCommand::Quit 로 정상 종료
    Quit,
    /// 연결 단절 (스트림 에러 또는 스트림 종료)
    Closed,
}

/// 서버에서 클라이언트로 보낸 CTCP 메세지 핸들링
fn handle_ctcp(client: &Client, source_nickname: &str, ctcp: CtcpCommand) {
    if let Some(reply) = get_ctcp_reply(&ctcp) {
        info!(event = "handle_ctcp_message", ctcp = ?ctcp, reply = %reply);
        let _ = client.send_notice(source_nickname, &reply);
    } else {
        if let CtcpCommand::Unknown(msg) = ctcp {
            warn!(event = "unknown_ctcp_command", message = %msg)
        }
    }
}

fn get_ctcp_reply(ctcp: &CtcpCommand) -> Option<String> {
    match ctcp {
        CtcpCommand::Version => Some("\x01VERSION kirc-rs v0.1\x01".to_string()),
        CtcpCommand::Ping(payload) => Some(format!("\x01PING {}\x01", payload)),
        CtcpCommand::Time => {
            let now = chrono::Local::now().to_rfc2822();
            Some(format!("\x01TIME {}\x01", now))
        }
        CtcpCommand::Unknown(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kirc::ctcp::CtcpCommand;

    #[test]
    fn test_get_ctcp_reply_version() {
        let reply = get_ctcp_reply(&CtcpCommand::Version);
        assert_eq!(reply, Some("\x01VERSION kirc-rs v0.1\x01".to_string()));
    }

    #[test]
    fn test_get_ctcp_reply_ping() {
        let reply = get_ctcp_reply(&CtcpCommand::Ping("12345".to_string()));
        assert_eq!(reply, Some("\x01PING 12345\x01".to_string()));
    }

    #[test]
    fn test_get_ctcp_reply_time() {
        let reply = get_ctcp_reply(&CtcpCommand::Time);
        assert!(reply.is_some());
        let reply_str = reply.unwrap();
        assert!(reply_str.starts_with("\x01TIME "));
        assert!(reply_str.ends_with("\x01"));
    }

    #[test]
    fn test_get_ctcp_reply_unknown() {
        let reply = get_ctcp_reply(&CtcpCommand::Unknown("FOO".to_string()));
        assert_eq!(reply, None);
    }
}
