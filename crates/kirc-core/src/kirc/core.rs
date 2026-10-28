use crate::kirc::ctcp::{parse_ctcp, CtcpCommand};
use crate::kirc::event::EventSender;
use crate::kirc::event::{
    emit_change_nick_failed, emit_server_status, emit_system_message, emit_ui_event,
};
use crate::kirc::state::kirc::KircState;
use crate::kirc::state::server::ServerState;
use crate::kirc::types::{ServerCommand, ServerId, ServerStatus};
use futures::prelude::*;
use irc::client::prelude::*;
use irc::client::ClientStream;
use irc::error;
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
use tokio::time::timeout;
use tracing::{debug, error, info, instrument, trace, warn};

/// IRC 서버 하나당 하나의 Actor. 연결 생명주기와 메시지 처리를 소유한다.
pub struct ServerActor {
    server_id: ServerId,
    server: Arc<ServerState>,
    ui_sink: EventSender,
    /// 스냅샷 저장용 (닉 변경 영속화). 액터 태스크가 상태 전체를 붙잡아 순환 참조가 되지
    /// 않도록 약한 참조로 둔다 — 종료 중이라 사라졌으면 저장을 건너뛴다.
    kirc_state: Weak<KircState>,
}

impl ServerActor {
    pub fn new(
        server_id: ServerId,
        server: Arc<ServerState>,
        ui_sink: EventSender,
        kirc_state: Weak<KircState>,
    ) -> Self {
        Self {
            server_id,
            server,
            ui_sink,
            kirc_state,
        }
    }

    /// 액터의 진입점. KircManager가 `tokio::spawn(actor.run())`으로 실행한다.
    #[instrument(name = "server_actor", skip_all, fields(server_id = %self.server_id))]
    pub async fn run(self) {
        debug!(server_id = %self.server_id, "Starting server actor");

        // 1. 연결 수립. 에러는 fail 로 소비
        let (client, stream) = match self.connect().await {
            Ok(connected) => connected,
            Err(e) => {
                self.fail(e.to_string());
                return;
            }
        };

        // 2. 이번 접속은 config의 닉으로 등록한다 — 이전 세션의 런타임 닉(강제 변경 등)이
        //    남아 있으면 실제 등록 닉과 어긋나므로 맞추고, 남은 닉 변경 요청은 버린다.
        self.server.cancel_nick_change();
        let config_nick = self.server.config().nickname().to_string();
        self.sync_own_nick(&config_nick);

        // 3. 커맨드 채널은 연결 성공 이후에 생성
        let (tx, rx) = unbounded_channel::<ServerCommand>();
        self.server.transition_to_registering(tx);
        emit_server_status(&self.ui_sink, self.server_id, ServerStatus::Registering);

        // 4. 메인 세션: IRC 스트림과 커맨드 채널을 select 로 소비
        trace!(server_id = %self.server_id, "Start server actor loop");
        let outcome = self.run_session(client, stream, rx).await;

        // 5. 세션 종료. 추후 재연결 구현 시 Closed 분기가 재시도 진입점이 된다.
        match &outcome {
            SessionOutcome::Failed(reason) => self.fail(reason.clone()),
            SessionOutcome::Quit => {
                self.server.transition_to_disconnected();
                emit_server_status(&self.ui_sink, self.server_id, ServerStatus::Disconnected);
            }
            SessionOutcome::Closed => {
                self.server.transition_to_disconnected();
                emit_server_status(&self.ui_sink, self.server_id, ServerStatus::Disconnected);
                // 사용자가 끊은 것이 아닌 연결 단절 — 서버 버퍼에 알림
                emit_system_message(
                    &self.ui_sink,
                    self.server_id,
                    "서버와의 연결이 끊어졌습니다.",
                );
            }
        }

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

    /// 메인 세션 루프. 정상 종료(Quit)·연결 단절(Closed)·실패(Failed)로 종료 사유를 반환한다.
    #[instrument(skip_all, fields(server_id = %self.server_id))]
    async fn run_session(
        &self,
        client: Client,
        mut stream: ClientStream,
        mut rx: UnboundedReceiver<ServerCommand>,
    ) -> SessionOutcome {
        // 등록 단계에서 닉이 거부될 때 시도한 대체 닉 수 (`alt_nick`)
        let mut alt_attempts = 0;
        loop {
            tokio::select! {
                // IRC 서버에서 오는 메시지
                result = stream.next() => {
                    match result {
                        Some(Ok(message)) => {
                            let _ = self.handle_message(&client, message);
                        }
                        Some(Err(error::Error::NoUsableNick)) => {
                            // 서버가 닉네임을 거부(433 ERR_NICKNAMEINUSE / 432 ERR_ERRONEOUSNICKNAME)했고
                            // alt_nicks가 비어 있어 irc crate가 더 시도할 닉네임이 없는 경우.
                            debug!(event = "irc_stream_error", error = "NoUsableNick", "IRC stream error, No usable nick");

                            // 등록 단계: 대체 닉(`닉_`, `닉__`, …)으로 다시 시도하고, 모두 거부되면
                            // 등록이 영영 끝나지 않으므로(Registering 정지) 세션을 실패 처리한다.
                            // irc crate의 alt_nicks는 등록 이후의 닉 변경 실패에도 대체 닉을 보내므로
                            // 쓰지 않고, 등록 단계에서만 직접 보낸다.
                            if self.server.status() == ServerStatus::Registering {
                                let nickname = self.server.config().nickname().to_string();
                                if alt_attempts < MAX_ALT_NICKS {
                                    alt_attempts += 1;
                                    let alt = alt_nick(&nickname, alt_attempts);
                                    info!(event = "alt_nick", nickname = %nickname, alt = %alt);
                                    if let Err(e) = client.send(Command::NICK(alt)) {
                                        error!("Failed to send alt nick: {e}");
                                    }
                                    continue;
                                }
                                let alts = (1..=MAX_ALT_NICKS)
                                    .map(|n| alt_nick(&nickname, n))
                                    .collect::<Vec<_>>()
                                    .join(", ");
                                return SessionOutcome::Failed(format!(
                                    "닉네임 '{nickname}'과(와) 대체 닉({alts})을 모두 사용할 수 없습니다 (이미 사용 중이거나 유효하지 않음). 닉네임을 바꿔 다시 연결하세요"
                                ));
                            }

                            // 닉네임 변경은 alt_nick 없이 실패시 emit 후 바로 continue
                            self.server.cancel_nick_change();
                            emit_change_nick_failed(&self.ui_sink, self.server_id, "Can't change nickname");

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
                            // irc crate는 소켓 쓰기(Outgoing)를 ClientStream::poll_next 안에서만
                            // 구동하므로, 바로 반환해 stream을 drop하면 QUIT가 flush되지 않는다.
                            // 서버가 QUIT에 응답해 연결을 닫을 때까지 stream을 계속 poll한다.
                            drain_after_quit(&mut stream).await;
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

    /// 런타임 현재 닉을 `nick`으로 맞추고, 달라졌으면 UI에도 NICK 이벤트로 알린다
    /// (UI의 `Server.nickname`이 내 메시지/JOIN 판정에 쓰이므로 함께 맞춰야 한다).
    fn sync_own_nick(&self, nick: &str) {
        let current = self.server.current_nickname();
        if current == nick {
            return;
        }
        self.server.set_current_nickname(nick);
        emit_ui_event(&self.ui_sink)
            .nick(self.server_id, current, nick.to_string())
            .emit();
    }

    /// 상태 스냅샷 저장 (상태가 이미 사라졌으면 건너뛴다)
    fn persist(&self) {
        if let Some(state) = self.kirc_state.upgrade() {
            state.persist();
        }
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
                    emit_ui_event(&self.ui_sink)
                        .user_message(self.server_id, target, source_nickname, content)
                        .emit();
                }
            }
            Command::JOIN(chanlist, _chankey, _real_name) => {
                emit_ui_event(&self.ui_sink)
                    .join(self.server_id, chanlist, source_nickname)
                    .emit();
            }
            Command::PART(chanlist, comment) => {
                emit_ui_event(&self.ui_sink)
                    .part(self.server_id, chanlist, source_nickname, comment)
                    .emit();
            }
            Command::QUIT(comment) => {
                emit_ui_event(&self.ui_sink)
                    .quit(self.server_id, source_nickname, comment)
                    .emit();
            }
            Command::NICK(nickname) => {
                // 1. 자기 자신인지 확인
                // IRC 닉은 대소문자를 구분하지 않는다
                if self
                    .server
                    .current_nickname()
                    .eq_ignore_ascii_case(&source_nickname)
                {
                    self.server.set_current_nickname(&nickname);
                    // 사용자가 요청한 변경이면 다음 접속에도 쓰도록 config에 저장
                    // (강제 변경은 이번 세션의 런타임 닉으로만 남는다)
                    if self.server.complete_nick_change(&nickname) {
                        info!(event = "nick_persisted", nickname = %nickname);
                        self.persist();
                    }
                }

                // 지금은 백엔드에서 유저목록 관리 x, 나중에 변경될 수 있음
                // 2. 모든 채널에서 유저 닉 변경
                /*for channel in state.channels.values_mut() {
                    if channel.users.remove(&old_nick) {
                        channel.users.insert(new_nick.clone());
                    }
                }*/

                // 3. 프론트로 이벤트 emit
                emit_ui_event(&self.ui_sink)
                    .nick(self.server_id, source_nickname, nickname)
                    .emit();
            }
            Command::TOPIC(channel, topic) => {
                emit_ui_event(&self.ui_sink)
                    .topic(self.server_id, channel, topic)
                    .emit();
            }
            Command::ERROR(message) => {
                warn!(message = %message, "Server command error");
                emit_ui_event(&self.ui_sink)
                    .error(self.server_id, message)
                    .emit();
            }
            Command::Response(Response::RPL_WELCOME, args) => {
                self.server.transition_to_connected();

                // 001의 첫 인자는 서버가 실제로 등록한 내 닉 (길이 제한으로 잘린 경우 등 반영)
                if let Some(registered) = args.first().filter(|nick| !nick.is_empty()) {
                    self.sync_own_nick(registered);

                    // 대체 닉(또는 서버가 바꾼 닉)으로 등록됐으면 서버 버퍼에 안내한다.
                    // 접속 닉(config)은 그대로라 다음 접속도 원래 닉으로 먼저 시도한다.
                    let config_nick = self.server.config().nickname().to_string();
                    if !registered.eq_ignore_ascii_case(&config_nick) {
                        emit_system_message(
                            &self.ui_sink,
                            self.server_id,
                            &format!(
                                "닉네임 '{config_nick}' 대신 '{registered}'(으)로 접속했습니다."
                            ),
                        );
                    }
                }

                // 기존 채널이 존재하면 연결
                for (channel_name, _channel_state) in self.server.channels() {
                    self.server
                        .send_command(ServerCommand::Join(channel_name))?;
                }

                emit_server_status(&self.ui_sink, self.server_id, ServerStatus::Connected);

                // Optional: Alert system message
                emit_system_message(&self.ui_sink, self.server_id, "서버에 연결되었습니다.");
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

        // 실패 사유를 서버 버퍼에도 남긴다 (Failed 상태의 last_error와 동일 문구)
        emit_system_message(
            &self.ui_sink,
            self.server_id,
            &format!("연결 실패: {message}"),
        );
        self.server.transition_to_failed(message);
        emit_server_status(&self.ui_sink, self.server_id, ServerStatus::Failed);
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
    /// 세션을 계속할 수 없는 실패 (예: 등록 단계 닉네임 거부). 사유는 Failed 상태에 기록된다.
    Failed(String),
}

/// 등록 단계에서 닉이 거부될 때 시도할 대체 닉 수 (`닉_`, `닉__`, `닉___`)
const MAX_ALT_NICKS: usize = 3;

/// `n`번째 대체 닉 — 원래 닉 뒤에 언더바를 `n`개 붙인다 (사용자 결정).
fn alt_nick(nickname: &str, n: usize) -> String {
    format!("{nickname}{}", "_".repeat(n))
}

/// QUIT 전송 후 서버가 연결을 닫을 때까지 대기하는 최대 시간.
/// `ServerRuntime::graceful_shutdown`의 액터 종료 대기(5s)보다 짧아야 한다.
const QUIT_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

/// QUIT 전송 후 stream을 poll해 큐잉된 QUIT를 실제로 소켓에 쓰고,
/// 서버가 연결을 닫을 때(None/Err)까지 남은 메시지를 버린다.
async fn drain_after_quit(stream: &mut ClientStream) {
    let drain = async {
        while let Some(result) = stream.next().await {
            match result {
                Ok(message) => trace!(command = ?message.command, "Discard message after QUIT"),
                Err(_) => break,
            }
        }
    };

    if timeout(QUIT_DRAIN_TIMEOUT, drain).await.is_err() {
        warn!(
            event = "quit_drain_timeout",
            "Server did not close connection after QUIT"
        );
    }
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

#[cfg(test)]
mod quit_tests {
    use super::*;
    use crate::kirc::event::UiEvent;
    use crate::kirc::manager::KircManager;
    use crate::kirc::persistence::ServerStateSnapshot;
    use crate::kirc::state::kirc::KircState;
    use crate::kirc::types::server::ServerConfig;
    use std::collections::HashMap;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    /// 001(RPL_WELCOME)로 등록을 끝내 주고, QUIT 수신 여부를 반환하는 mock IRC 서버.
    async fn spawn_mock_server() -> (u16, tokio::task::JoinHandle<bool>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let handle = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let (read, mut write) = socket.into_split();
            let mut lines = BufReader::new(read).lines();

            while let Ok(Some(line)) = lines.next_line().await {
                if line.starts_with("QUIT") {
                    // 실제 서버처럼 QUIT에 응답하고 연결을 닫는다
                    let _ = write.write_all(b"ERROR :Closing link\r\n").await;
                    return true;
                }
                if line.starts_with("USER") {
                    let _ = write.write_all(b":mock 001 nick :Welcome\r\n").await;
                }
            }
            false
        });

        (port, handle)
    }

    async fn wait_for_status(
        rx: &mut UnboundedReceiver<UiEvent>,
        server_id: ServerId,
        status: ServerStatus,
    ) {
        timeout(Duration::from_secs(5), async {
            loop {
                match rx.recv().await {
                    Some(UiEvent::ServerStatus(p))
                        if p.server_id == server_id && p.status == status =>
                    {
                        return;
                    }
                    Some(_) => continue,
                    None => panic!("event channel closed while waiting for {status:?}"),
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {status:?}"));
    }

    /// disconnect 시 QUIT가 실제로 소켓까지 flush되는지 (mock IRC 서버로 검증)
    #[tokio::test]
    async fn quit_is_flushed_to_server_on_disconnect() {
        let (port, mock) = spawn_mock_server().await;

        let config = ServerConfig::new("127.0.0.1".to_string(), port, false, "nick".to_string());
        let state = Arc::new(KircState::from_iter([ServerStateSnapshot::new(
            config.clone(),
            HashMap::new(),
        )]));
        let server_id = *state.get_all_servers().keys().next().unwrap();

        let (tx, mut rx) = unbounded_channel::<UiEvent>();
        let manager = KircManager::new(state.clone(), Arc::new(tx));

        manager.connect_server(Some(server_id), config).unwrap();
        wait_for_status(&mut rx, server_id, ServerStatus::Connected).await;

        manager.disconnect_server(server_id).unwrap();

        let got_quit = timeout(Duration::from_secs(5), mock)
            .await
            .expect("mock server timed out")
            .unwrap();
        assert!(got_quit, "QUIT was never written to the socket");

        wait_for_status(&mut rx, server_id, ServerStatus::Disconnected).await;
    }

    /// 등록 단계에서 닉네임이 중복(433)되면 Registering에 갇히지 않고 사유와 함께 Failed가 된다.
    #[tokio::test]
    async fn nick_in_use_during_registration_fails_server() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let mock = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let (read, mut write) = socket.into_split();
            let mut lines = BufReader::new(read).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line.starts_with("NICK") {
                    let _ = write
                        .write_all(b":mock 433 * nick :Nickname is already in use\r\n")
                        .await;
                }
            }
        });

        let config = ServerConfig::new("127.0.0.1".to_string(), port, false, "nick".to_string());
        let state = Arc::new(KircState::from_iter([ServerStateSnapshot::new(
            config.clone(),
            HashMap::new(),
        )]));
        let server_id = *state.get_all_servers().keys().next().unwrap();
        let server = state.get_server(server_id).unwrap();

        let (tx, mut rx) = unbounded_channel::<UiEvent>();
        let manager = KircManager::new(state.clone(), Arc::new(tx));

        manager.connect_server(Some(server_id), config).unwrap();

        // 실패 사유는 Failed 상태 직전에 서버 버퍼용 SystemMessage로도 전달된다
        let reason_message = timeout(Duration::from_secs(5), async {
            loop {
                match rx.recv().await {
                    Some(UiEvent::SystemMessage(p)) if p.server_id == server_id => {
                        return p.message;
                    }
                    Some(_) => continue,
                    None => panic!("event channel closed"),
                }
            }
        })
        .await
        .expect("no failure system message");
        assert!(
            reason_message.starts_with("연결 실패: "),
            "{reason_message}"
        );
        assert!(reason_message.contains("'nick'"), "{reason_message}");
        wait_for_status(&mut rx, server_id, ServerStatus::Failed).await;

        let error = server.last_error().expect("failure reason missing");
        assert!(error.contains("'nick'"), "unexpected reason: {error}");

        // Failed 이후 Disconnected로 덮어쓰지 않는다
        tokio::task::yield_now().await;
        assert_eq!(server.status(), ServerStatus::Failed);

        mock.abort();
    }

    /// 디스크의 config.json을 다시 읽어 채널명 → locked 맵으로 돌려준다.
    fn load_saved_channels(path: &std::path::Path) -> HashMap<String, bool> {
        use crate::kirc::persistence::KircStateSnapshot;
        use crate::memento::Memento;

        let snapshot: KircStateSnapshot = crate::fs::load(path).unwrap();
        let state: KircState = snapshot.restore();
        state
            .get_all_servers()
            .values()
            .flat_map(|server| server.channels())
            .map(|(name, channel)| (name, channel.locked))
            .collect()
    }

    /// lock/unlock/join/part는 정상 종료를 기다리지 않고 즉시 config.json에 반영된다.
    #[tokio::test]
    async fn channel_changes_are_persisted_immediately() {
        use crate::kirc::commands::{ChannelPayload, KircApi};

        let (port, _mock) = spawn_mock_server().await;
        let path = std::env::temp_dir().join(format!("kirc-test-{}.json", uuid::Uuid::now_v7()));

        let config = ServerConfig::new("127.0.0.1".to_string(), port, false, "nick".to_string());
        let mut state =
            KircState::from_iter([ServerStateSnapshot::new(config.clone(), HashMap::new())]);
        state.set_persistence_path(&path);
        let state = Arc::new(state);
        let server_id = *state.get_all_servers().keys().next().unwrap();

        let (tx, mut rx) = unbounded_channel::<UiEvent>();
        let sink: EventSender = Arc::new(tx);
        let api = KircApi::new(Arc::new(KircManager::new(state.clone(), sink.clone())));

        api.connect_server(crate::kirc::commands::ConnectServerPayload::new(
            Some(server_id),
            "127.0.0.1".to_string(),
            port,
            false,
            "nick".to_string(),
        ))
        .unwrap();
        wait_for_status(&mut rx, server_id, ServerStatus::Connected).await;

        let payload = ChannelPayload::new(server_id, "#test".to_string());

        api.join_channel(server_id, "#test").unwrap();
        assert_eq!(load_saved_channels(&path).get("#test"), Some(&false));

        api.lock_channel(&payload, &sink).unwrap();
        assert_eq!(load_saved_channels(&path).get("#test"), Some(&true));

        api.unlock_channel(&payload, &sink).unwrap();
        assert_eq!(load_saved_channels(&path).get("#test"), Some(&false));

        api.leave_channel(&payload).unwrap();
        assert_eq!(load_saved_channels(&path).get("#test"), None);

        let _ = std::fs::remove_file(&path);
    }

    // ==================== 닉네임 영속화/동기화 ====================

    /// 닉 변경 시나리오용 mock IRC 서버. 연결을 여러 번 받는다(재접속 검증용).
    /// - `welcome_nick`: 001에 넣을 닉 (None이면 클라이언트가 등록한 닉 그대로)
    /// - `forced_nick`: 001 직후 서버가 강제로 바꿀 닉 (NickServ 강제 변경 흉내)
    ///
    /// 받은 줄은 `"{연결 번호}:{줄}"`로 기록한다.
    async fn spawn_nick_mock(
        welcome_nick: Option<&'static str>,
        forced_nick: Option<&'static str>,
    ) -> (u16, Arc<std::sync::Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let lines = Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = lines.clone();

        tokio::spawn(async move {
            let mut conn = 0;
            while let Ok((socket, _)) = listener.accept().await {
                let (read, mut write) = socket.into_split();
                let mut reader = BufReader::new(read).lines();
                let mut current = String::new();
                let mut registered = false;

                while let Ok(Some(line)) = reader.next_line().await {
                    log.lock().unwrap().push(format!("{conn}:{line}"));
                    if line.starts_with("QUIT") {
                        let _ = write.write_all(b"ERROR :Closing link\r\n").await;
                        break;
                    }
                    if let Some(nick) = line.strip_prefix("NICK ") {
                        if registered {
                            // 등록 이후 닉 변경 요청 → 수락
                            let reply = format!(":{current}!u@mock NICK :{nick}\r\n");
                            let _ = write.write_all(reply.as_bytes()).await;
                        }
                        current = nick.to_string();
                    }
                    if line.starts_with("USER") && !registered {
                        registered = true;
                        if let Some(welcome) = welcome_nick {
                            current = welcome.to_string();
                        }
                        let reply = format!(":mock 001 {current} :Welcome\r\n");
                        let _ = write.write_all(reply.as_bytes()).await;
                        if let Some(forced) = forced_nick {
                            let reply = format!(":{current}!u@mock NICK :{forced}\r\n");
                            let _ = write.write_all(reply.as_bytes()).await;
                            current = forced.to_string();
                        }
                    }
                }
                conn += 1;
            }
        });

        (port, lines)
    }

    /// mock 서버에 연결된 매니저/API 한 벌 (config.json은 임시 경로)
    struct NickHarness {
        api: crate::kirc::commands::KircApi,
        state: Arc<KircState>,
        server_id: ServerId,
        rx: UnboundedReceiver<UiEvent>,
        path: std::path::PathBuf,
    }

    impl NickHarness {
        fn new(port: u16) -> Self {
            let path =
                std::env::temp_dir().join(format!("kirc-test-{}.json", uuid::Uuid::now_v7()));
            let config =
                ServerConfig::new("127.0.0.1".to_string(), port, false, "nick".to_string());
            let mut state =
                KircState::from_iter([ServerStateSnapshot::new(config, HashMap::new())]);
            state.set_persistence_path(&path);
            let state = Arc::new(state);
            let server_id = *state.get_all_servers().keys().next().unwrap();

            let (tx, rx) = unbounded_channel::<UiEvent>();
            let manager = Arc::new(KircManager::new(state.clone(), Arc::new(tx)));
            Self {
                api: crate::kirc::commands::KircApi::new(manager),
                state,
                server_id,
                rx,
                path,
            }
        }

        fn connect(&self) {
            let config = self.state.get_server(self.server_id).unwrap().config();
            self.api
                .manager()
                .connect_server(Some(self.server_id), config)
                .unwrap();
        }

        fn server(&self) -> Arc<ServerState> {
            self.state.get_server(self.server_id).unwrap()
        }

        /// 디스크에 저장된 접속 닉 (아직 저장된 적 없으면 None)
        fn saved_nick(&self) -> Option<String> {
            use crate::kirc::persistence::KircStateSnapshot;
            use crate::memento::Memento;
            if !self.path.exists() {
                return None;
            }
            let snapshot: KircStateSnapshot = crate::fs::load(&self.path).unwrap();
            let state: KircState = snapshot.restore();
            let server = state.get_all_servers().into_values().next()?;
            let nick = server.config().nickname().to_string();
            Some(nick)
        }

        /// 다음 Nick UI 이벤트를 기다린다 (old, new)
        async fn next_nick_event(&mut self) -> (String, String) {
            timeout(Duration::from_secs(5), async {
                loop {
                    match self.rx.recv().await {
                        Some(UiEvent::Ui(crate::kirc::event::UIEventPayload::Nick {
                            old_nick,
                            new_nick,
                            ..
                        })) => return (old_nick, new_nick),
                        Some(_) => continue,
                        None => panic!("event channel closed"),
                    }
                }
            })
            .await
            .expect("no nick event")
        }
    }

    impl Drop for NickHarness {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    /// 요청한 닉 변경은 즉시 config.json에 저장되고, 다시 접속하면 새 닉으로 등록한다.
    /// (이전: 런타임 닉만 바뀌어 재시작/재접속 시 이전 닉으로 등록을 시도했다)
    #[tokio::test]
    async fn requested_nick_change_is_persisted_and_used_on_reconnect() {
        let (port, lines) = spawn_nick_mock(None, None).await;
        let mut h = NickHarness::new(port);

        h.connect();
        wait_for_status(&mut h.rx, h.server_id, ServerStatus::Connected).await;

        let payload = crate::kirc::commands::ChangeNickPayload::new(h.server_id, "newnick".into());
        h.api.change_nickname(&payload).unwrap();
        assert_eq!(h.next_nick_event().await, ("nick".into(), "newnick".into()));

        assert_eq!(h.server().current_nickname(), "newnick");
        assert_eq!(h.server().config().nickname(), "newnick");
        assert_eq!(h.saved_nick().as_deref(), Some("newnick"));

        // 다시 접속하면 새 닉으로 등록
        h.api.disconnect_server(h.server_id).unwrap();
        wait_for_status(&mut h.rx, h.server_id, ServerStatus::Disconnected).await;
        h.connect();
        wait_for_status(&mut h.rx, h.server_id, ServerStatus::Connected).await;

        let lines = lines.lock().unwrap().clone();
        assert!(lines.contains(&"1:NICK newnick".to_string()), "{lines:?}");
    }

    /// 요청과 무관한 강제 변경(NickServ 등)은 런타임 닉만 바꾸고 저장하지 않는다.
    #[tokio::test]
    async fn forced_nick_change_is_not_persisted() {
        let (port, _lines) = spawn_nick_mock(None, Some("Guest123")).await;
        let mut h = NickHarness::new(port);

        h.connect();
        assert_eq!(
            h.next_nick_event().await,
            ("nick".into(), "Guest123".into())
        );

        assert_eq!(h.server().current_nickname(), "Guest123");
        assert_eq!(h.server().config().nickname(), "nick");
        assert_eq!(h.saved_nick(), None);
    }

    /// 001에 담긴 실제 등록 닉으로 런타임 닉과 UI를 맞추고, 재접속 시 이전 세션의 런타임 닉은
    /// config 닉으로 되돌린다.
    #[tokio::test]
    async fn runtime_nick_syncs_with_registration() {
        // 서버가 등록 닉을 바꿔서 받아 줌 (예: 길이 제한으로 잘림)
        let (port, _lines) = spawn_nick_mock(Some("nick_"), None).await;
        let mut h = NickHarness::new(port);

        h.connect();
        assert_eq!(h.next_nick_event().await, ("nick".into(), "nick_".into()));
        assert_eq!(h.server().current_nickname(), "nick_");
        // 서버가 바꾼 것이지 요청한 변경이 아니므로 config는 그대로
        assert_eq!(h.server().config().nickname(), "nick");

        // 재접속: 등록 전 런타임 닉을 config 닉으로 되돌린다 → UI에도 알림
        h.api.disconnect_server(h.server_id).unwrap();
        wait_for_status(&mut h.rx, h.server_id, ServerStatus::Disconnected).await;
        h.connect();
        assert_eq!(h.next_nick_event().await, ("nick_".into(), "nick".into()));
    }

    // ==================== 닉 선점 대응 (대체 닉 / 연결 없이 닉 변경) ====================

    /// 등록 시 `taken`에 있는 닉은 433으로 거부하는 mock IRC 서버 (연결 여러 번 수락).
    async fn spawn_taken_nick_mock(
        taken: &'static [&'static str],
    ) -> (u16, Arc<std::sync::Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let lines = Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = lines.clone();

        tokio::spawn(async move {
            let mut conn = 0;
            while let Ok((socket, _)) = listener.accept().await {
                let (read, mut write) = socket.into_split();
                let mut reader = BufReader::new(read).lines();
                let mut nick: Option<String> = None;
                let mut user = false;
                let mut registered = false;

                while let Ok(Some(line)) = reader.next_line().await {
                    log.lock().unwrap().push(format!("{conn}:{line}"));
                    if line.starts_with("QUIT") {
                        let _ = write.write_all(b"ERROR :Closing link\r\n").await;
                        break;
                    }
                    if let Some(requested) = line.strip_prefix("NICK ") {
                        if taken.contains(&requested) {
                            let reply =
                                format!(":mock 433 * {requested} :Nickname is already in use\r\n");
                            let _ = write.write_all(reply.as_bytes()).await;
                        } else {
                            nick = Some(requested.to_string());
                        }
                    }
                    if line.starts_with("USER") {
                        user = true;
                    }
                    if !registered && user {
                        if let Some(nick) = &nick {
                            registered = true;
                            let reply = format!(":mock 001 {nick} :Welcome\r\n");
                            let _ = write.write_all(reply.as_bytes()).await;
                        }
                    }
                }
                conn += 1;
            }
        });

        (port, lines)
    }

    /// 다음 서버 버퍼(System) 메시지를 기다린다
    async fn next_system_message(rx: &mut UnboundedReceiver<UiEvent>) -> String {
        timeout(Duration::from_secs(5), async {
            loop {
                match rx.recv().await {
                    Some(UiEvent::SystemMessage(p)) => return p.message,
                    Some(_) => continue,
                    None => panic!("event channel closed"),
                }
            }
        })
        .await
        .expect("no system message")
    }

    /// 접속 닉이 선점돼 있으면 `닉_`으로 접속한다. 대체 닉은 저장하지 않는다.
    #[tokio::test]
    async fn taken_nick_falls_back_to_underscore_alt() {
        let (port, lines) = spawn_taken_nick_mock(&["nick"]).await;
        let mut h = NickHarness::new(port);

        h.connect();
        assert_eq!(h.next_nick_event().await, ("nick".into(), "nick_".into()));
        wait_for_status(&mut h.rx, h.server_id, ServerStatus::Connected).await;

        assert_eq!(h.server().current_nickname(), "nick_");
        assert_eq!(h.server().config().nickname(), "nick");
        assert_eq!(h.saved_nick(), None);
        assert!(lines.lock().unwrap().contains(&"0:NICK nick_".to_string()));
    }

    /// 대체 닉(`닉_`~`닉___`)까지 모두 선점돼 있으면 사유와 함께 Failed.
    #[tokio::test]
    async fn all_alt_nicks_taken_fails_with_reason() {
        let (port, _lines) = spawn_taken_nick_mock(&["nick", "nick_", "nick__", "nick___"]).await;
        let mut h = NickHarness::new(port);

        h.connect();
        wait_for_status(&mut h.rx, h.server_id, ServerStatus::Failed).await;
        let reason = h.server().last_error().unwrap();
        assert!(reason.contains("nick___"), "{reason}");
    }

    /// 연결 안 된(Disconnected) 서버에서 닉을 바꾸면 접속 닉을 바로 저장하고, 다시 연결하지 않는다.
    #[tokio::test]
    async fn offline_nick_change_saves_without_connecting() {
        let (port, lines) = spawn_taken_nick_mock(&[]).await;
        let mut h = NickHarness::new(port);

        let payload = crate::kirc::commands::ChangeNickPayload::new(h.server_id, "newnick".into());
        h.api.change_nickname(&payload).unwrap();

        assert_eq!(h.next_nick_event().await, ("nick".into(), "newnick".into()));
        assert_eq!(h.server().config().nickname(), "newnick");
        assert_eq!(h.saved_nick().as_deref(), Some("newnick"));
        assert_eq!(h.server().status(), ServerStatus::Disconnected);
        assert!(lines.lock().unwrap().is_empty(), "should not connect");
    }

    /// 닉 선점으로 접속에 실패(Failed)한 서버에서 닉을 바꾸면 새 닉으로 바로 다시 연결한다.
    /// (이전: 연결이 없어 "Server not connected"로 닉을 바꿀 수 없었다)
    #[tokio::test]
    async fn offline_nick_change_reconnects_failed_server() {
        let (port, lines) = spawn_taken_nick_mock(&["nick", "nick_", "nick__", "nick___"]).await;
        let mut h = NickHarness::new(port);

        h.connect();
        wait_for_status(&mut h.rx, h.server_id, ServerStatus::Failed).await;

        let payload = crate::kirc::commands::ChangeNickPayload::new(h.server_id, "free".into());
        h.api.change_nickname(&payload).unwrap();
        wait_for_status(&mut h.rx, h.server_id, ServerStatus::Connected).await;

        assert_eq!(h.server().current_nickname(), "free");
        assert_eq!(h.saved_nick().as_deref(), Some("free"));
        assert!(lines.lock().unwrap().contains(&"1:NICK free".to_string()));
    }

    /// 대체 닉으로 접속하면 서버 버퍼에 안내를 남긴다.
    #[tokio::test]
    async fn alt_nick_connection_is_announced() {
        let (port, _lines) = spawn_taken_nick_mock(&["nick"]).await;
        let mut h = NickHarness::new(port);

        h.connect();
        let message = next_system_message(&mut h.rx).await;
        assert!(message.contains("'nick_'"), "{message}");
    }
}
