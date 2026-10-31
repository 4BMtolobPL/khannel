use crate::client::ctcp::{CtcpCommand, ctcp_body, parse_ctcp};
use crate::client::event::{EventSender, MemberModeChange, NamesEntry};
use crate::client::event::{
    emit_change_nick_failed, emit_server_status, emit_system_message, emit_ui_event,
};
use crate::client::state::khannel::KhannelState;
use crate::client::state::server::ServerState;
use crate::client::types::{ServerCommand, ServerId, ServerStatus};
use futures::prelude::*;
use irc::client::ClientStream;
use irc::client::prelude::*;
use irc::error;
use irc::proto::mode::ModeType;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Weak};
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tokio::time::timeout;
use tracing::{debug, error, info, instrument, trace, warn};

/// IRC 서버 하나당 하나의 Actor. 연결 생명주기와 메시지 처리를 소유한다.
pub struct ServerActor {
    server_id: ServerId,
    server: Arc<ServerState>,
    ui_sink: EventSender,
    /// 스냅샷 저장용 (닉 변경 영속화). 액터 태스크가 상태 전체를 붙잡아 순환 참조가 되지
    /// 않도록 약한 참조로 둔다 — 종료 중이라 사라졌으면 저장을 건너뛴다.
    khannel_state: Weak<KhannelState>,
}

impl ServerActor {
    pub fn new(
        server_id: ServerId,
        server: Arc<ServerState>,
        ui_sink: EventSender,
        khannel_state: Weak<KhannelState>,
    ) -> Self {
        Self {
            server_id,
            server,
            ui_sink,
            khannel_state,
        }
    }

    /// 액터의 진입점. KhannelManager가 `tokio::spawn(actor.run())`으로 실행한다.
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
        let mut ctx = SessionCtx::default();
        loop {
            tokio::select! {
                // IRC 서버에서 오는 메시지
                result = stream.next() => {
                    match result {
                        Some(Ok(message)) => {
                            let _ = self.handle_message(&client, &mut ctx, message);
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
                                if ctx.alt_attempts < MAX_ALT_NICKS {
                                    ctx.alt_attempts += 1;
                                    let alt = alt_nick(&nickname, ctx.alt_attempts);
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
                        Some(cmd) => self.handle_server_command(&client, &mut ctx, cmd),
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
    fn handle_server_command(&self, client: &Client, ctx: &mut SessionCtx, cmd: ServerCommand) {
        match cmd {
            ServerCommand::Join(ch) => {
                info!(event = "join", channel = %ch);
                if let Err(e) = client.send_join(&ch) {
                    error!("Failed to send join message: {e}");
                }
                ctx.pending_joins.insert(ch.to_lowercase());
            }
            ServerCommand::Privmsg { target, message } => {
                if let Err(e) = client.send_privmsg(&target, &message) {
                    error!("Failed to send privmsg: {e}");
                }

                // 자신이 보낸 PRIVMSG를 UI에 반영하기 위해 에코 메시지로 다시 처리
                self.send_echo_message(client, ctx, &target, &message);
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

    fn send_echo_message(
        &self,
        client: &Client,
        ctx: &mut SessionCtx,
        target: &str,
        message: &str,
    ) {
        let current_nick = self.current_nick();

        match Message::with_tags(None, Some(&current_nick), "PRIVMSG", vec![target, message]) {
            Ok(msg) => {
                if let Err(e) = self.handle_message(client, ctx, msg) {
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
        if let Some(state) = self.khannel_state.upgrade() {
            state.persist();
        }
    }

    #[instrument(skip_all, fields(server_id = %self.server_id))]
    fn handle_message(
        &self,
        client: &Client,
        ctx: &mut SessionCtx,
        message: Message,
    ) -> anyhow::Result<()> {
        let source_nickname = message.source_nickname().unwrap_or("").to_string();

        match message.command {
            Command::PRIVMSG(target, content) => match parse_ctcp(&content) {
                Some(CtcpCommand::Action(action)) => {
                    // `/me` — CTCP지만 응답하지 않고 대화 메시지로 표시한다
                    emit_ui_event(&self.ui_sink)
                        .action_message(self.server_id, target, source_nickname, action)
                        .emit();
                }
                Some(ctcp) => {
                    info!(target = %target, content = %content, "Received CTCP message");
                    handle_ctcp(client, &source_nickname, ctcp);
                }
                None => {
                    emit_ui_event(&self.ui_sink)
                        .user_message(self.server_id, target, source_nickname, content)
                        .emit();
                }
            },
            Command::NOTICE(target, content) => {
                // 서버 발신(접두사가 서버명이거나 없음)이면 닉이 없다
                let nick = Some(source_nickname).filter(|nick| !nick.is_empty());
                if let Some(body) = ctcp_body(&content) {
                    // NOTICE의 CTCP는 내가 보낸 CTCP 요청의 응답 — 표시만 하고 절대 응답하지 않는다
                    info!(target = %target, content = %content, "Received CTCP reply");
                    emit_system_message(
                        &self.ui_sink,
                        self.server_id,
                        &format!("CTCP reply from {}: {body}", nick.as_deref().unwrap_or("*")),
                    );
                } else {
                    emit_ui_event(&self.ui_sink)
                        .notice(self.server_id, target, nick, content)
                        .emit();
                }
            }
            Command::JOIN(chanlist, _chankey, _real_name) => {
                // 내 입장이 확인됐으면 더 이상 실패를 기다리지 않는다
                if self
                    .server
                    .current_nickname()
                    .eq_ignore_ascii_case(&source_nickname)
                {
                    ctx.pending_joins.remove(&chanlist.to_lowercase());
                }
                emit_ui_event(&self.ui_sink)
                    .join(self.server_id, chanlist, source_nickname)
                    .emit();
            }
            Command::PART(chanlist, comment) => {
                emit_ui_event(&self.ui_sink)
                    .part(self.server_id, chanlist, source_nickname, comment)
                    .emit();
            }
            Command::ChannelMODE(channel, modes) => {
                // 서버가 건 모드(입장 직후 +nt 등)는 발신자 닉이 없다
                if !modes.is_empty() {
                    let by = Some(source_nickname).filter(|nick| !nick.is_empty());
                    let member_changes = member_mode_changes(&modes, ctx.prefix());
                    emit_ui_event(&self.ui_sink)
                        .channel_mode(
                            self.server_id,
                            channel,
                            by,
                            format_modes(&modes),
                            member_changes,
                        )
                        .emit();
                }
            }
            Command::UserMODE(target, modes) => {
                // 내 유저 모드 (접속 직후 +i 등) — 서버 버퍼에 알린다
                if !modes.is_empty() {
                    emit_system_message(
                        &self.ui_sink,
                        self.server_id,
                        &format!("Mode {target} {}", format_modes(&modes)),
                    );
                }
            }
            Command::INVITE(invited, channel) => {
                let by = Some(source_nickname)
                    .filter(|nick| !nick.is_empty())
                    .unwrap_or_else(|| "Server".to_string());
                // invite-notify가 켜진 서버에선 다른 사람이 초대된 것도 알려준다
                let message = if self
                    .server
                    .current_nickname()
                    .eq_ignore_ascii_case(&invited)
                {
                    format!("{by} invited you to {channel}")
                } else {
                    format!("{by} invited {invited} to {channel}")
                };
                emit_system_message(&self.ui_sink, self.server_id, &message);
            }
            Command::Response(Response::RPL_ISUPPORT, args) => {
                // args: [내 닉, "TOKEN=값", ..., "are supported by this server"] — 여러 줄로 온다
                if let Some(prefix) = args
                    .iter()
                    .skip(1)
                    .find_map(|token| parse_isupport_prefix(token))
                {
                    debug!(event = "isupport_prefix", modes = %prefix.modes, symbols = %prefix.symbols);
                    ctx.prefix = Some(prefix);
                }
            }
            Command::KICK(channel, kicked, comment) => {
                // 발신자가 강퇴한 사람. 내가 강퇴돼도 저장된 채널 목록은 그대로 둔다
                // → 재접속 시 자동으로 다시 입장한다 (irssi/weechat 기본 동작과 같음)
                let comment = comment.filter(|comment| !comment.is_empty());
                emit_ui_event(&self.ui_sink)
                    .kick(self.server_id, channel, kicked, source_nickname, comment)
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
                // 누군가 토픽을 바꾼 경우. 빈 토픽은 토픽 삭제
                let topic = topic.filter(|topic| !topic.is_empty());
                emit_ui_event(&self.ui_sink)
                    .topic(self.server_id, channel, topic, Some(source_nickname))
                    .emit();
            }
            Command::Response(Response::RPL_TOPIC, args) => {
                // 입장 시(또는 TOPIC 조회 시) 현재 토픽. args: [내 닉, 채널, 토픽]
                if let [_, channel, topic, ..] = args.as_slice() {
                    let topic = Some(topic.clone()).filter(|topic| !topic.is_empty());
                    emit_ui_event(&self.ui_sink)
                        .topic(self.server_id, channel.clone(), topic, None)
                        .emit();
                }
            }
            Command::Response(Response::RPL_NOTOPIC, args) => {
                // args: [내 닉, 채널, "No topic is set"]
                if let Some(channel) = args.get(1) {
                    emit_ui_event(&self.ui_sink)
                        .topic(self.server_id, channel.clone(), None, None)
                        .emit();
                }
            }
            Command::Response(Response::RPL_TOPICWHOTIME, args) => {
                // args: [내 닉, 채널, 설정자, 설정 시각(unix)] — 아직 UI에 표시하지 않는다
                trace!(args = ?args, "Topic set by");
            }
            Command::ERROR(message) => {
                warn!(message = %message, "Server command error");
                emit_ui_event(&self.ui_sink)
                    .error(self.server_id, None, message)
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

                // 기존 채널이 존재하면 연결. 커맨드 채널(ServerCommand::Join)을 거치지 않고 바로
                // 보낸다 — 사용자 요청 입장(pending_joins)과 구분해, 자동 재입장이 실패해도
                // 저장된 채널을 지우지 않기 위함
                for (channel_name, _channel_state) in self.server.channels() {
                    info!(event = "auto_join", channel = %channel_name);
                    if let Err(e) = client.send_join(&channel_name) {
                        error!("Failed to send join message: {e}");
                    }
                }

                emit_server_status(&self.ui_sink, self.server_id, ServerStatus::Connected);

                // Optional: Alert system message
                emit_system_message(&self.ui_sink, self.server_id, "서버에 연결되었습니다.");
            }
            Command::Response(Response::RPL_NAMREPLY, args) => {
                // args: [내 닉, 채널 타입(= * @), 채널, "닉 @닉 +닉 ..."]
                // 한 채널의 목록이 여러 줄로 나뉘어 오므로 366까지 모은다
                if let [_, .., channel, names] = args.as_slice() {
                    let symbols = ctx.prefix().symbols.clone();
                    ctx.pending_names
                        .entry(channel.to_lowercase())
                        .or_default()
                        .extend(
                            names
                                .split_whitespace()
                                .map(|entry| split_nick_prefix(entry, &symbols))
                                .filter(|(_, nick)| !nick.is_empty())
                                .map(|(prefixes, nick)| NamesEntry {
                                    nick: nick.to_string(),
                                    prefixes: prefixes.to_string(),
                                }),
                        );
                }
            }
            Command::Response(Response::RPL_ENDOFNAMES, args) => {
                // args: [내 닉, 채널, "End of /NAMES list."]
                // 모은 목록으로 UI의 유저 목록을 통째로 교체한다 (재요청/재접속 시 중복 방지)
                if let Some(channel) = args.get(1) {
                    let users = ctx
                        .pending_names
                        .remove(&channel.to_lowercase())
                        .unwrap_or_default();
                    let symbols = ctx.prefix().symbols.clone();
                    emit_ui_event(&self.ui_sink)
                        .names(self.server_id, channel.clone(), users, symbols)
                        .emit();
                }
            }
            Command::Response(Response::ERR_NICKNAMEINUSE, _) => {
                // 닉네임이 중복된 경우
                // alt nick으로 재시도
                // 그래도 안될경우 오류 던지기
                trace!("닉네임 중복");
            }
            Command::Response(Response::RPL_MOTDSTART, args) => {
                // 이전 MOTD가 끝나지 않았어도 새로 시작한다
                ctx.motd = Some(args.last().into_iter().cloned().collect());
            }
            Command::Response(Response::RPL_MOTD, args) => {
                // 줄마다 붙는 "- " 접두사는 뗀다. 375 없이 와도 모은다
                if let Some(line) = args.last() {
                    let line = line.strip_prefix("- ").unwrap_or(line);
                    ctx.motd.get_or_insert_with(Vec::new).push(line.to_string());
                }
            }
            Command::Response(Response::RPL_ENDOFMOTD, _) => {
                // 수십 줄짜리 MOTD가 서버 버퍼를 덮지 않도록 메시지 하나로 보낸다
                if let Some(lines) = ctx.motd.take().filter(|lines| !lines.is_empty()) {
                    emit_system_message(&self.ui_sink, self.server_id, &lines.join("\n"));
                }
            }
            Command::Response(response, args) if is_info_numeric(response) => {
                if let Some(text) = info_text(response, &args) {
                    emit_system_message(&self.ui_sink, self.server_id, &text);
                }
            }
            Command::Response(response, args) if is_join_error(response) => {
                self.handle_join_error(ctx, &args);
            }
            Command::Response(response, args) if is_target_error(response) => {
                self.emit_numeric_error(&args);
            }
            _ => {
                // TODO: Command 다른것도 추가하기
                debug!(event = "unprocessed_irc_message", command = ?message.command);
            }
        }

        Ok(())
    }

    /// JOIN 실패 numeric. args: [내 닉, 채널, 사유]
    ///
    /// 이번 세션에 사용자가 요청한 입장이면 저장된 채널을 지운다 — 남겨 두면 재접속할 때마다
    /// 자동 재입장이 같은 이유로 실패한다. 자동 재입장(001 이후)이 실패한 경우는 밴/초대 전용
    /// 같은 일시적 사유일 수 있으므로 알리기만 하고 채널은 유지한다.
    fn handle_join_error(&self, ctx: &mut SessionCtx, args: &[String]) {
        let [_, channel, .., reason] = args else {
            self.emit_numeric_error(args);
            return;
        };
        if !ctx.pending_joins.remove(&channel.to_lowercase()) {
            self.emit_numeric_error(args);
            return;
        }

        warn!(event = "join_failed", channel = %channel, reason = %reason);
        if self.server.remove_channel(channel).is_some() {
            self.persist();
        }
        emit_ui_event(&self.ui_sink)
            .join_failed(self.server_id, channel.clone(), reason.clone())
            .emit();
    }

    /// 대상(채널/닉)이 있는 에러 numeric을 UI 에러로 보낸다. args: [내 닉, 대상, 사유]
    fn emit_numeric_error(&self, args: &[String]) {
        let (target, reason) = match args {
            [_, target, .., reason] => (Some(target.clone()), reason.clone()),
            [.., reason] => (None, reason.clone()),
            [] => return,
        };
        debug!(event = "numeric_error", target = ?target, reason = %reason);
        emit_ui_event(&self.ui_sink)
            .error(self.server_id, target, reason)
            .emit();
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

/// 한 세션(연결) 동안만 유지되는 액터 지역 상태. 재접속하면 새로 만든다.
#[derive(Default)]
struct SessionCtx {
    /// 등록 단계에서 닉이 거부될 때 시도한 대체 닉 수 (`alt_nick`)
    alt_attempts: usize,
    /// 366(RPL_ENDOFNAMES)까지 모으는 채널별 NAMES(353) 응답. 키는 소문자 채널명
    pending_names: HashMap<String, Vec<NamesEntry>>,
    /// 사용자가 요청했고 아직 JOIN 응답이 오지 않은 채널 (소문자). 자동 재입장은 포함하지 않는다
    pending_joins: HashSet<String>,
    /// 376(RPL_ENDOFMOTD)까지 모으는 MOTD 줄 (375 머리줄 포함)
    motd: Option<Vec<String>>,
    /// 005 PREFIX로 받은 채널 권한 모드/접두사 (예: `(ov)@+`). 받기 전엔 None
    prefix: Option<PrefixMap>,
}

impl SessionCtx {
    /// 서버의 권한 모드/접두사. 005를 받기 전이면 흔히 쓰이는 기본값
    fn prefix(&self) -> &PrefixMap {
        self.prefix.as_ref().unwrap_or(&DEFAULT_PREFIX)
    }
}

/// 서버 버퍼에 그대로 보여줄 접속 정보 numeric (MOTD는 따로 모은다)
fn is_info_numeric(response: Response) -> bool {
    matches!(
        response,
        Response::RPL_YOURHOST
            | Response::RPL_CREATED
            | Response::RPL_LUSERCLIENT
            | Response::RPL_LUSEROP
            | Response::RPL_LUSERUNKNOWN
            | Response::RPL_LUSERCHANNELS
            | Response::RPL_LUSERME
            | Response::RPL_LOCALUSERS
            | Response::RPL_GLOBALUSERS
            | Response::ERR_NOMOTD
    )
}

/// 접속 정보 numeric의 표시 문구. 첫 인자(내 닉)는 뺀다.
fn info_text(response: Response, args: &[String]) -> Option<String> {
    match response {
        // args: [내 닉, 개수, "operator(s) online"] → 개수와 설명을 합친다
        Response::RPL_LUSEROP | Response::RPL_LUSERUNKNOWN | Response::RPL_LUSERCHANNELS => {
            Some(args.get(1..)?.join(" ")).filter(|text| !text.is_empty())
        }
        // 그 외는 마지막 인자가 사람이 읽는 문장이다
        // (265/266은 앞에 숫자 인자가 더 붙지만 문장에 이미 들어 있다)
        _ => args.get(1..)?.last().cloned(),
    }
}

/// 채널 입장 실패 numeric
fn is_join_error(response: Response) -> bool {
    matches!(
        response,
        Response::ERR_NOSUCHCHANNEL
            | Response::ERR_TOOMANYCHANNELS
            | Response::ERR_CHANNELISFULL
            | Response::ERR_INVITEONLYCHAN
            | Response::ERR_BANNEDFROMCHAN
            | Response::ERR_BADCHANNELKEY
            | Response::ERR_BADCHANMASK
            // RFC상 ERR_NOCHANMODES지만 Libera 등에서는 "등록된 닉만 입장 가능(+r)"
            | Response::ERR_NOCHANMODES
    )
}

/// 대상(채널/닉)에 대한 요청이 실패했음을 알리는 numeric
fn is_target_error(response: Response) -> bool {
    matches!(
        response,
        Response::ERR_NOSUCHNICK
            | Response::ERR_CANNOTSENDTOCHAN
            | Response::ERR_USERNOTINCHANNEL
            | Response::ERR_NOTONCHANNEL
            | Response::ERR_CHANOPRIVSNEEDED
    )
}

/// 채널 권한 모드 문자와 접두사 기호의 대응 (005 PREFIX). 같은 위치끼리 짝이고, 높은 권한이 먼저 온다
#[derive(Debug, Clone, PartialEq, Eq)]
struct PrefixMap {
    /// 모드 문자 (예: `ov`)
    modes: String,
    /// 접두사 기호 (예: `@+`)
    symbols: String,
}

impl PrefixMap {
    /// 권한 모드 문자(`o`)에 대응하는 접두사 기호(`@`)
    fn symbol_for(&self, mode: char) -> Option<char> {
        let index = self.modes.chars().position(|m| m == mode)?;
        self.symbols.chars().nth(index)
    }
}

/// 005 PREFIX를 받기 전에 쓰는 채널 권한 모드/접두사 (흔히 쓰이는 것 전부)
static DEFAULT_PREFIX: LazyLock<PrefixMap> = LazyLock::new(|| PrefixMap {
    modes: "qaohv".to_string(),
    symbols: "~&@%+".to_string(),
});

/// NAMES 응답 항목을 (권한 접두사, 닉)으로 나눈다. multi-prefix(`@+nick`)도 처리.
fn split_nick_prefix<'a>(entry: &'a str, symbols: &str) -> (&'a str, &'a str) {
    let nick = entry.trim_start_matches(|c| symbols.contains(c));
    (&entry[..entry.len() - nick.len()], nick)
}

/// 005(RPL_ISUPPORT) 토큰 중 `PREFIX=(ov)@+`를 모드/기호로 꺼낸다.
/// `PREFIX=`(값 없음)는 권한 접두사가 없다는 뜻이라 둘 다 빈 문자열이다.
fn parse_isupport_prefix(token: &str) -> Option<PrefixMap> {
    let value = token.strip_prefix("PREFIX=")?;
    if value.is_empty() {
        return Some(PrefixMap {
            modes: String::new(),
            symbols: String::new(),
        });
    }
    let (modes, symbols) = value.strip_prefix('(')?.split_once(')')?;
    Some(PrefixMap {
        modes: modes.to_string(),
        symbols: symbols.to_string(),
    })
}

/// 채널 MODE 중 유저 권한 변경(`+o nick`, `-v nick` ...)만 골라 접두사 기호 변경으로 바꾼다
fn member_mode_changes(modes: &[Mode<ChannelMode>], prefix: &PrefixMap) -> Vec<MemberModeChange> {
    modes
        .iter()
        .filter_map(|mode| {
            let (added, nick) = match mode {
                Mode::Plus(_, Some(nick)) => (true, nick),
                Mode::Minus(_, Some(nick)) => (false, nick),
                _ => return None,
            };
            // flag()는 `+o` 형태 — 부호 다음 글자가 모드 문자
            let letter = mode.flag().chars().nth(1)?;
            let symbol = prefix.symbol_for(letter)?;
            Some(MemberModeChange {
                nick: nick.clone(),
                symbol,
                added,
            })
        })
        .collect()
}

/// MODE 변경을 `+nt`, `+o-v nick1 nick2` 형태로 (같은 부호는 묶고, 모드 문자 다음에 인자들)
fn format_modes<T: ModeType>(modes: &[Mode<T>]) -> String {
    let mut flags = String::new();
    let mut sign = None;
    for flag in modes.iter().map(Mode::flag) {
        match flag.chars().next() {
            Some(s @ ('+' | '-')) => {
                if sign != Some(s) {
                    flags.push(s);
                    sign = Some(s);
                }
                flags.push_str(&flag[1..]);
            }
            _ => flags.push_str(&flag),
        }
    }
    modes
        .iter()
        .filter_map(Mode::arg)
        .fold(flags, |mut acc, arg| {
            acc.push(' ');
            acc.push_str(arg);
            acc
        })
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
        CtcpCommand::Version => Some(format!(
            "\x01VERSION khannel v{}\x01",
            env!("CARGO_PKG_VERSION")
        )),
        CtcpCommand::Ping(payload) => Some(format!("\x01PING {}\x01", payload)),
        CtcpCommand::Time => {
            let now = chrono::Local::now().to_rfc2822();
            Some(format!("\x01TIME {}\x01", now))
        }
        // ACTION은 handle_message에서 메시지로 처리한다
        CtcpCommand::Action(_) | CtcpCommand::Unknown(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::ctcp::CtcpCommand;

    #[test]
    fn test_get_ctcp_reply_version() {
        let reply = get_ctcp_reply(&CtcpCommand::Version);
        assert_eq!(
            reply,
            Some(format!(
                "\x01VERSION khannel v{}\x01",
                env!("CARGO_PKG_VERSION")
            ))
        );
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

    fn args(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn test_info_text() {
        assert_eq!(
            info_text(
                Response::RPL_LUSERCLIENT,
                &args(&["nick", "There are 5 users"])
            )
            .as_deref(),
            Some("There are 5 users")
        );
        assert_eq!(
            info_text(
                Response::RPL_LUSEROP,
                &args(&["nick", "3", "operator(s) online"])
            )
            .as_deref(),
            Some("3 operator(s) online")
        );
        assert_eq!(
            info_text(
                Response::RPL_LOCALUSERS,
                &args(&["nick", "10", "20", "Current local users 10, max 20"])
            )
            .as_deref(),
            Some("Current local users 10, max 20")
        );
        // 내 닉만 있고 내용이 없으면 표시하지 않는다
        assert_eq!(info_text(Response::RPL_LUSEROP, &args(&["nick"])), None);
        assert_eq!(info_text(Response::RPL_CREATED, &args(&["nick"])), None);
    }

    #[test]
    fn test_split_nick_prefix() {
        let symbols = DEFAULT_PREFIX.symbols.as_str();
        assert_eq!(split_nick_prefix("nick", symbols), ("", "nick"));
        assert_eq!(split_nick_prefix("@op", symbols), ("@", "op"));
        assert_eq!(split_nick_prefix("+voice", symbols), ("+", "voice"));
        assert_eq!(split_nick_prefix("~owner", symbols), ("~", "owner"));
        assert_eq!(split_nick_prefix("@+both", symbols), ("@+", "both"));
        assert_eq!(split_nick_prefix("@", symbols), ("@", ""));
        // 005 PREFIX=(ov)@+ 서버에선 ~/&로 시작하는 닉을 그대로 둔다
        assert_eq!(split_nick_prefix("~tilde", "@+"), ("", "~tilde"));
        assert_eq!(split_nick_prefix("@op", ""), ("", "@op"));
    }

    fn prefix_map(modes: &str, symbols: &str) -> PrefixMap {
        PrefixMap {
            modes: modes.to_string(),
            symbols: symbols.to_string(),
        }
    }

    #[test]
    fn test_parse_isupport_prefix() {
        assert_eq!(
            parse_isupport_prefix("PREFIX=(qaohv)~&@%+"),
            Some(prefix_map("qaohv", "~&@%+"))
        );
        assert_eq!(
            parse_isupport_prefix("PREFIX=(ov)@+"),
            Some(prefix_map("ov", "@+"))
        );
        assert_eq!(parse_isupport_prefix("PREFIX="), Some(prefix_map("", "")));
        assert_eq!(parse_isupport_prefix("CHANTYPES=#"), None);
        assert_eq!(parse_isupport_prefix("PREFIX=garbage"), None);
    }

    #[test]
    fn test_member_mode_changes() {
        let change = |nick: &str, symbol, added| MemberModeChange {
            nick: nick.to_string(),
            symbol,
            added,
        };

        // 권한 모드만 고르고, 인자 없는 모드(+n)나 권한이 아닌 인자 모드(+b)는 뺀다
        let modes = Mode::as_channel_modes(&["+o-v+nb", "lee", "bob", "*!*@spam"]).unwrap();
        assert_eq!(
            member_mode_changes(&modes, &DEFAULT_PREFIX),
            [change("lee", '@', true), change("bob", '+', false)]
        );

        // 서버 PREFIX에 없는 권한 모드는 무시한다 (PREFIX=(ov)@+ 서버의 +h)
        let modes = Mode::as_channel_modes(&["+hq", "a", "b"]).unwrap();
        assert!(member_mode_changes(&modes, &prefix_map("ov", "@+")).is_empty());
        assert_eq!(
            member_mode_changes(&modes, &DEFAULT_PREFIX),
            [change("a", '%', true), change("b", '~', true)]
        );
    }

    #[test]
    fn test_format_modes() {
        let modes = Mode::as_channel_modes(&["+o-v", "lee", "bob"]).unwrap();
        assert_eq!(format_modes(&modes), "+o-v lee bob");
        let modes = Mode::as_channel_modes(&["+nt"]).unwrap();
        assert_eq!(format_modes(&modes), "+nt");
        let modes = Mode::as_channel_modes(&["+o-v+b", "a", "b", "c!*@*"]).unwrap();
        assert_eq!(format_modes(&modes), "+o-v+b a b c!*@*");
    }
}

#[cfg(test)]
mod quit_tests {
    use super::*;
    use crate::client::event::UiEvent;
    use crate::client::manager::KhannelManager;
    use crate::client::persistence::ServerStateSnapshot;
    use crate::client::state::khannel::KhannelState;
    use crate::client::types::server::ServerConfig;
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
        let state = Arc::new(KhannelState::from_iter([ServerStateSnapshot::new(
            config.clone(),
            HashMap::new(),
        )]));
        let server_id = *state.get_all_servers().keys().next().unwrap();

        let (tx, mut rx) = unbounded_channel::<UiEvent>();
        let manager = KhannelManager::new(state.clone(), Arc::new(tx));

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
        let state = Arc::new(KhannelState::from_iter([ServerStateSnapshot::new(
            config.clone(),
            HashMap::new(),
        )]));
        let server_id = *state.get_all_servers().keys().next().unwrap();
        let server = state.get_server(server_id).unwrap();

        let (tx, mut rx) = unbounded_channel::<UiEvent>();
        let manager = KhannelManager::new(state.clone(), Arc::new(tx));

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
        use crate::client::persistence::KhannelStateSnapshot;
        use crate::memento::Memento;

        let snapshot: KhannelStateSnapshot = crate::fs::load(path).unwrap();
        let state: KhannelState = snapshot.restore();
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
        use crate::client::commands::{ChannelPayload, KhannelApi};

        let (port, _mock) = spawn_mock_server().await;
        let path = std::env::temp_dir().join(format!("khannel-test-{}.json", uuid::Uuid::now_v7()));

        let config = ServerConfig::new("127.0.0.1".to_string(), port, false, "nick".to_string());
        let mut state =
            KhannelState::from_iter([ServerStateSnapshot::new(config.clone(), HashMap::new())]);
        state.set_persistence_path(&path);
        let state = Arc::new(state);
        let server_id = *state.get_all_servers().keys().next().unwrap();

        let (tx, mut rx) = unbounded_channel::<UiEvent>();
        let sink: EventSender = Arc::new(tx);
        let api = KhannelApi::new(Arc::new(KhannelManager::new(state.clone(), sink.clone())));

        api.connect_server(crate::client::commands::ConnectServerPayload::new(
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
        api: crate::client::commands::KhannelApi,
        state: Arc<KhannelState>,
        server_id: ServerId,
        rx: UnboundedReceiver<UiEvent>,
        path: std::path::PathBuf,
    }

    impl NickHarness {
        fn new(port: u16) -> Self {
            let path =
                std::env::temp_dir().join(format!("khannel-test-{}.json", uuid::Uuid::now_v7()));
            let config =
                ServerConfig::new("127.0.0.1".to_string(), port, false, "nick".to_string());
            let mut state =
                KhannelState::from_iter([ServerStateSnapshot::new(config, HashMap::new())]);
            state.set_persistence_path(&path);
            let state = Arc::new(state);
            let server_id = *state.get_all_servers().keys().next().unwrap();

            let (tx, rx) = unbounded_channel::<UiEvent>();
            let manager = Arc::new(KhannelManager::new(state.clone(), Arc::new(tx)));
            Self {
                api: crate::client::commands::KhannelApi::new(manager),
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
            use crate::client::persistence::KhannelStateSnapshot;
            use crate::memento::Memento;
            if !self.path.exists() {
                return None;
            }
            let snapshot: KhannelStateSnapshot = crate::fs::load(&self.path).unwrap();
            let state: KhannelState = snapshot.restore();
            let server = state.get_all_servers().into_values().next()?;
            let nick = server.config().nickname().to_string();
            Some(nick)
        }

        /// 다음 Nick UI 이벤트를 기다린다 (old, new)
        async fn next_nick_event(&mut self) -> (String, String) {
            timeout(Duration::from_secs(5), async {
                loop {
                    match self.rx.recv().await {
                        Some(UiEvent::Ui(crate::client::event::UIEventPayload::Nick {
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

        let payload =
            crate::client::commands::ChangeNickPayload::new(h.server_id, "newnick".into());
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
                    if !registered
                        && user
                        && let Some(nick) = &nick
                    {
                        registered = true;
                        let reply = format!(":mock 001 {nick} :Welcome\r\n");
                        let _ = write.write_all(reply.as_bytes()).await;
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

        let payload =
            crate::client::commands::ChangeNickPayload::new(h.server_id, "newnick".into());
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

        let payload = crate::client::commands::ChangeNickPayload::new(h.server_id, "free".into());
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

    // ==================== NAMES ====================

    /// 001로 등록을 끝내 주고, JOIN을 받으면 `replies`를 그대로 돌려주는 mock IRC 서버.
    async fn spawn_join_reply_mock(replies: &'static str) -> u16 {
        spawn_logging_join_reply_mock(replies).await.0
    }

    /// [`spawn_join_reply_mock`]과 같고, 클라이언트가 보낸 줄을 기록해 함께 돌려준다.
    async fn spawn_logging_join_reply_mock(
        replies: &'static str,
    ) -> (u16, Arc<std::sync::Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let sent = Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = sent.clone();

        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let (read, mut write) = socket.into_split();
            let mut lines = BufReader::new(read).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                log.lock().unwrap().push(line.clone());
                if line.starts_with("USER") {
                    let _ = write.write_all(b":mock 001 nick :Welcome\r\n").await;
                }
                if line.starts_with("JOIN") {
                    let _ = write.write_all(replies.as_bytes()).await;
                }
            }
        });

        (port, sent)
    }

    /// 표식 메시지를 보내고 mock 서버에 도착할 때까지 기다린 뒤, 그때까지 받은 줄을 돌려준다.
    /// 송신은 순서대로 나가므로 그 전에 생긴 응답(중복 포함)은 모두 표식보다 먼저 도착해 있다.
    async fn lines_until_marker(
        h: &NickHarness,
        sent: &std::sync::Mutex<Vec<String>>,
    ) -> Vec<String> {
        h.api
            .send_message(h.server_id, "#marker", "marker")
            .unwrap();
        timeout(Duration::from_secs(5), async {
            loop {
                let lines = sent.lock().unwrap().clone();
                if lines.iter().any(|line| line.starts_with("PRIVMSG #marker")) {
                    return lines;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("marker never reached the server")
    }

    /// 여러 줄의 353을 366까지 모아, 닉과 접두사를 나눈 전체 목록으로 한 번에 보낸다.
    #[tokio::test]
    async fn names_reply_is_collected_until_end_of_names() {
        let port = spawn_join_reply_mock(concat!(
            ":mock 353 nick = #Test :nick @op\r\n",
            ":mock 353 nick = #Test :+voice ~owner @+both\r\n",
            ":mock 366 nick #Test :End of /NAMES list.\r\n",
        ))
        .await;
        let mut h = NickHarness::new(port);

        h.connect();
        wait_for_status(&mut h.rx, h.server_id, ServerStatus::Connected).await;
        h.api.join_channel(h.server_id, "#test").unwrap();

        let (channel, users) = timeout(Duration::from_secs(5), async {
            loop {
                match h.rx.recv().await {
                    Some(UiEvent::Ui(crate::client::event::UIEventPayload::Names {
                        channel,
                        users,
                        ..
                    })) => return (channel, users),
                    Some(_) => continue,
                    None => panic!("event channel closed"),
                }
            }
        })
        .await
        .expect("no names event");

        assert_eq!(channel, "#Test");
        let users: Vec<(&str, &str)> = users
            .iter()
            .map(|u| (u.prefixes.as_str(), u.nick.as_str()))
            .collect();
        assert_eq!(
            users,
            [
                ("", "nick"),
                ("@", "op"),
                ("+", "voice"),
                ("~", "owner"),
                ("@+", "both")
            ]
        );
    }

    // ==================== TOPIC ====================

    /// 다음 Topic UI 이벤트를 기다린다 (channel, topic, changed_by)
    async fn next_topic_event(
        rx: &mut UnboundedReceiver<UiEvent>,
    ) -> (String, Option<String>, Option<String>) {
        timeout(Duration::from_secs(5), async {
            loop {
                match rx.recv().await {
                    Some(UiEvent::Ui(crate::client::event::UIEventPayload::Topic {
                        channel,
                        topic,
                        changed_by,
                        ..
                    })) => return (channel, topic, changed_by),
                    Some(_) => continue,
                    None => panic!("event channel closed"),
                }
            }
        })
        .await
        .expect("no topic event")
    }

    /// 입장 시 받은 현재 토픽(332/331)은 변경자 없이, TOPIC 커맨드는 변경자와 함께 보낸다.
    #[tokio::test]
    async fn topic_numerics_and_topic_change_are_distinguished() {
        let port = spawn_join_reply_mock(concat!(
            ":mock 332 nick #a :hello world\r\n",
            ":mock 333 nick #a op 1700000000\r\n",
            ":mock 331 nick #b :No topic is set\r\n",
            ":op!u@mock TOPIC #a :new topic\r\n",
            ":op!u@mock TOPIC #a :\r\n",
        ))
        .await;
        let mut h = NickHarness::new(port);

        h.connect();
        wait_for_status(&mut h.rx, h.server_id, ServerStatus::Connected).await;
        h.api.join_channel(h.server_id, "#a").unwrap();

        assert_eq!(
            next_topic_event(&mut h.rx).await,
            ("#a".into(), Some("hello world".into()), None)
        );
        assert_eq!(next_topic_event(&mut h.rx).await, ("#b".into(), None, None));
        assert_eq!(
            next_topic_event(&mut h.rx).await,
            ("#a".into(), Some("new topic".into()), Some("op".into()))
        );
        assert_eq!(
            next_topic_event(&mut h.rx).await,
            ("#a".into(), None, Some("op".into()))
        );
    }

    // ==================== NOTICE ====================

    /// NOTICE는 대상/보낸 닉(서버면 None)과 함께 보내고, NOTICE 안의 CTCP 응답은
    /// 서버 버퍼 시스템 메시지로만 표시한다.
    #[tokio::test]
    async fn notices_are_forwarded_and_ctcp_replies_shown() {
        let port = spawn_join_reply_mock(concat!(
            ":irc.mock.net NOTICE * :*** server notice\r\n",
            ":NickServ!s@services NOTICE nick :please identify\r\n",
            ":op!u@mock NOTICE #a :channel notice\r\n",
            ":bob!u@mock NOTICE nick :\x01VERSION bobirc 1.0\x01\r\n",
        ))
        .await;
        let mut h = NickHarness::new(port);

        h.connect();
        wait_for_status(&mut h.rx, h.server_id, ServerStatus::Connected).await;
        h.api.join_channel(h.server_id, "#a").unwrap();

        let mut notices = Vec::new();
        let ctcp = timeout(Duration::from_secs(5), async {
            loop {
                match h.rx.recv().await {
                    Some(UiEvent::Ui(crate::client::event::UIEventPayload::Notice {
                        target,
                        nick,
                        content,
                        ..
                    })) => notices.push((target, nick, content)),
                    Some(UiEvent::SystemMessage(p)) if p.message.starts_with("CTCP") => {
                        return p.message;
                    }
                    Some(_) => continue,
                    None => panic!("event channel closed"),
                }
            }
        })
        .await
        .expect("no ctcp reply message");

        assert_eq!(
            notices,
            [
                ("*".into(), None, "*** server notice".into()),
                (
                    "nick".into(),
                    Some("NickServ".into()),
                    "please identify".into()
                ),
                ("#a".into(), Some("op".into()), "channel notice".into()),
            ]
        );
        assert_eq!(ctcp, "CTCP reply from bob: VERSION bobirc 1.0");
    }

    // ==================== CTCP ACTION ====================

    /// `/me`(CTCP ACTION)는 응답하지 않고 action 메시지로 보낸다. 일반 메시지는 action=false.
    #[tokio::test]
    async fn ctcp_action_is_forwarded_as_action_message() {
        let port = spawn_join_reply_mock(concat!(
            ":bob!u@mock PRIVMSG #a :\x01ACTION waves\x01\r\n",
            ":bob!u@mock PRIVMSG #a :hello\r\n",
        ))
        .await;
        let mut h = NickHarness::new(port);

        h.connect();
        wait_for_status(&mut h.rx, h.server_id, ServerStatus::Connected).await;
        h.api.join_channel(h.server_id, "#a").unwrap();

        let mut messages = Vec::new();
        timeout(Duration::from_secs(5), async {
            while messages.len() < 2 {
                match h.rx.recv().await {
                    Some(UiEvent::Ui(crate::client::event::UIEventPayload::UserMessage {
                        nick,
                        content,
                        action,
                        ..
                    })) => messages.push((nick, content, action)),
                    Some(_) => continue,
                    None => panic!("event channel closed"),
                }
            }
        })
        .await
        .expect("no user messages");

        assert_eq!(
            messages,
            [
                ("bob".into(), "waves".into(), true),
                ("bob".into(), "hello".into(), false),
            ]
        );
    }

    // ==================== KICK ====================

    /// KICK은 강퇴당한 닉/강퇴한 닉/사유로 보내고, 내가 강퇴돼도 저장된 채널은 유지한다
    /// (재접속 시 자동 재입장).
    #[tokio::test]
    async fn kick_is_forwarded_and_keeps_saved_channel() {
        let port = spawn_join_reply_mock(concat!(
            ":op!u@mock KICK #a lee :spam\r\n",
            ":op!u@mock KICK #a nick\r\n",
        ))
        .await;
        let mut h = NickHarness::new(port);

        h.connect();
        wait_for_status(&mut h.rx, h.server_id, ServerStatus::Connected).await;
        h.api.join_channel(h.server_id, "#a").unwrap();

        let mut kicks = Vec::new();
        timeout(Duration::from_secs(5), async {
            while kicks.len() < 2 {
                match h.rx.recv().await {
                    Some(UiEvent::Ui(crate::client::event::UIEventPayload::Kick {
                        channel,
                        nick,
                        by,
                        reason,
                        ..
                    })) => kicks.push((channel, nick, by, reason)),
                    Some(_) => continue,
                    None => panic!("event channel closed"),
                }
            }
        })
        .await
        .expect("no kick events");

        assert_eq!(
            kicks,
            [
                ("#a".into(), "lee".into(), "op".into(), Some("spam".into())),
                ("#a".into(), "nick".into(), "op".into(), None),
            ]
        );
        assert!(h.server().contains_channel("#a"));
    }

    // ==================== 에러 numeric ====================

    /// 001로 등록을 끝내 주고, 모든 JOIN을 474(밴)로 거부하는 mock IRC 서버.
    async fn spawn_join_reject_mock() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let (read, mut write) = socket.into_split();
            let mut lines = BufReader::new(read).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line.starts_with("USER") {
                    let _ = write.write_all(b":mock 001 nick :Welcome\r\n").await;
                }
                if let Some(channel) = line.strip_prefix("JOIN ") {
                    let reply = format!(":mock 474 nick {channel} :Cannot join channel (+b)\r\n");
                    let _ = write.write_all(reply.as_bytes()).await;
                }
            }
        });

        port
    }

    /// 다음 JoinFailed 또는 Error UI 이벤트를 기다린다
    async fn next_error_event(
        rx: &mut UnboundedReceiver<UiEvent>,
    ) -> crate::client::event::UIEventPayload {
        use crate::client::event::UIEventPayload as P;
        timeout(Duration::from_secs(5), async {
            loop {
                match rx.recv().await {
                    Some(UiEvent::Ui(payload @ (P::JoinFailed { .. } | P::Error { .. }))) => {
                        return payload;
                    }
                    Some(_) => continue,
                    None => panic!("event channel closed"),
                }
            }
        })
        .await
        .expect("no error event")
    }

    /// 사용자가 요청한 입장이 실패하면 저장된 채널을 지우고 JoinFailed를 보낸다.
    #[tokio::test]
    async fn requested_join_failure_removes_saved_channel() {
        use crate::client::event::UIEventPayload as P;
        let port = spawn_join_reject_mock().await;
        let mut h = NickHarness::new(port);

        h.connect();
        wait_for_status(&mut h.rx, h.server_id, ServerStatus::Connected).await;
        h.api.join_channel(h.server_id, "#banned").unwrap();
        assert!(h.server().contains_channel("#banned"));

        match next_error_event(&mut h.rx).await {
            P::JoinFailed {
                channel, reason, ..
            } => {
                assert_eq!(channel, "#banned");
                assert_eq!(reason, "Cannot join channel (+b)");
            }
            other => panic!("unexpected event: {other:?}"),
        }
        assert!(!h.server().contains_channel("#banned"));
        assert_eq!(load_saved_channels(&h.path).get("#banned"), None);
    }

    /// 접속 시 자동 재입장이 실패하면 알리기만 하고 저장된 채널은 유지한다.
    #[tokio::test]
    async fn auto_rejoin_failure_keeps_saved_channel() {
        use crate::client::event::UIEventPayload as P;
        let port = spawn_join_reject_mock().await;
        let mut h = NickHarness::new(port);
        h.server().insert_channel("#saved", true);

        h.connect();
        match next_error_event(&mut h.rx).await {
            P::Error {
                target, message, ..
            } => {
                assert_eq!(target.as_deref(), Some("#saved"));
                assert_eq!(message, "Cannot join channel (+b)");
            }
            other => panic!("unexpected event: {other:?}"),
        }
        assert!(h.server().contains_channel("#saved"));
    }

    /// 대상이 있는 에러(404 등)는 대상과 사유를 담은 Error로 보낸다.
    #[tokio::test]
    async fn target_error_numeric_is_forwarded_with_target() {
        use crate::client::event::UIEventPayload as P;
        let port = spawn_join_reply_mock(":mock 404 nick #a :Cannot send to channel\r\n").await;
        let mut h = NickHarness::new(port);

        h.connect();
        wait_for_status(&mut h.rx, h.server_id, ServerStatus::Connected).await;
        h.api.join_channel(h.server_id, "#a").unwrap();

        match next_error_event(&mut h.rx).await {
            P::Error {
                target, message, ..
            } => {
                assert_eq!(target.as_deref(), Some("#a"));
                assert_eq!(message, "Cannot send to channel");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    // ==================== irc 크레이트 기본 동작과의 중복 ====================

    /// CTCP 요청에는 보낸 사람에게 한 번만 응답한다. 채널로 온 요청도 채널이 아닌 보낸 사람에게.
    /// (이전: irc 크레이트의 ctcp feature도 응답해 중복됐고, 채널 요청엔 채널 전체로 응답했다)
    #[tokio::test]
    async fn ctcp_requests_get_a_single_reply_to_sender() {
        let (port, sent) = spawn_logging_join_reply_mock(concat!(
            ":bob!u@mock PRIVMSG nick :\x01VERSION\x01\r\n",
            ":bob!u@mock PRIVMSG #a :\x01PING 123\x01\r\n",
        ))
        .await;
        let mut h = NickHarness::new(port);

        h.connect();
        wait_for_status(&mut h.rx, h.server_id, ServerStatus::Connected).await;
        h.api.join_channel(h.server_id, "#a").unwrap();
        // 두 응답이 다 나간 뒤에 표식을 보낸다
        timeout(Duration::from_secs(5), async {
            while sent
                .lock()
                .unwrap()
                .iter()
                .filter(|line| line.starts_with("NOTICE"))
                .count()
                < 2
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("ctcp replies not sent");

        let notices: Vec<String> = lines_until_marker(&h, &sent)
            .await
            .into_iter()
            .filter(|line| line.starts_with("NOTICE"))
            .collect();
        assert_eq!(notices.len(), 2, "{notices:?}");
        assert!(notices[0].starts_with("NOTICE bob "), "{notices:?}");
        assert!(notices[0].contains("VERSION khannel"), "{notices:?}");
        assert!(notices[1].starts_with("NOTICE bob "), "{notices:?}");
        assert!(notices[1].contains("PING 123"), "{notices:?}");
    }

    /// 접속 후 저장된 채널에는 JOIN을 한 번만 보낸다.
    /// (이전: irc 크레이트의 channel-lists feature가 376에서 입장한 채널로 JOIN을 다시 보냈다)
    #[tokio::test]
    async fn saved_channel_is_joined_once_after_motd() {
        let (port, sent) = spawn_logging_join_reply_mock(concat!(
            ":nick!u@mock JOIN #saved\r\n",
            ":mock 353 nick = #saved :nick\r\n",
            ":mock 366 nick #saved :End of /NAMES list.\r\n",
            ":mock 376 nick :End of /MOTD command.\r\n",
        ))
        .await;
        let mut h = NickHarness::new(port);
        h.server().insert_channel("#saved", false);

        h.connect();
        // 376까지 처리된 뒤(= MOTD 메시지 없이 끝나므로 NAMES 이벤트 이후) 표식을 보낸다
        timeout(Duration::from_secs(5), async {
            loop {
                if let Some(UiEvent::Ui(crate::client::event::UIEventPayload::Names { .. })) =
                    h.rx.recv().await
                {
                    return;
                }
            }
        })
        .await
        .expect("no names event");

        let joins: Vec<String> = lines_until_marker(&h, &sent)
            .await
            .into_iter()
            .filter(|line| line.starts_with("JOIN"))
            .collect();
        assert_eq!(joins, ["JOIN #saved"]);
    }

    // ==================== ISUPPORT / MODE / INVITE ====================

    /// 005 PREFIX를 받은 뒤엔 그 기호만 NAMES에서 뗀다 (`~`로 시작하는 닉을 보존).
    #[tokio::test]
    async fn names_use_isupport_prefix_symbols() {
        let port = spawn_join_reply_mock(concat!(
            ":mock 005 nick PREFIX=(ov)@+ CHANTYPES=# :are supported by this server\r\n",
            ":mock 353 nick = #a :@op ~tilde\r\n",
            ":mock 366 nick #a :End of /NAMES list.\r\n",
        ))
        .await;
        let mut h = NickHarness::new(port);

        h.connect();
        wait_for_status(&mut h.rx, h.server_id, ServerStatus::Connected).await;
        h.api.join_channel(h.server_id, "#a").unwrap();

        let (users, prefix_symbols) = timeout(Duration::from_secs(5), async {
            loop {
                if let Some(UiEvent::Ui(crate::client::event::UIEventPayload::Names {
                    users,
                    prefix_symbols,
                    ..
                })) = h.rx.recv().await
                {
                    return (users, prefix_symbols);
                }
            }
        })
        .await
        .expect("no names event");
        let users: Vec<(&str, &str)> = users
            .iter()
            .map(|u| (u.prefixes.as_str(), u.nick.as_str()))
            .collect();
        assert_eq!(users, [("@", "op"), ("", "~tilde")]);
        assert_eq!(prefix_symbols, "@+");
    }

    /// 채널 MODE는 바꾼 닉(서버면 None)과 함께, INVITE는 서버 버퍼 메시지로 보낸다.
    #[tokio::test]
    async fn channel_mode_and_invite_are_forwarded() {
        let port = spawn_join_reply_mock(concat!(
            ":irc.mock.net MODE #a +nt\r\n",
            ":op!u@mock MODE #a +o-v lee bob\r\n",
            ":bob!u@mock INVITE nick #secret\r\n",
        ))
        .await;
        let mut h = NickHarness::new(port);

        h.connect();
        wait_for_status(&mut h.rx, h.server_id, ServerStatus::Connected).await;
        assert_eq!(
            next_system_message(&mut h.rx).await,
            "서버에 연결되었습니다."
        );
        h.api.join_channel(h.server_id, "#a").unwrap();

        let mut modes = Vec::new();
        let invite = timeout(Duration::from_secs(5), async {
            loop {
                match h.rx.recv().await {
                    Some(UiEvent::Ui(crate::client::event::UIEventPayload::ChannelMode {
                        channel,
                        by,
                        modes: changed,
                        member_changes,
                        ..
                    })) => modes.push((channel, by, changed, member_changes.len())),
                    Some(UiEvent::SystemMessage(p)) => return p.message,
                    Some(_) => continue,
                    None => panic!("event channel closed"),
                }
            }
        })
        .await
        .expect("no invite message");

        assert_eq!(
            modes,
            [
                ("#a".into(), None, "+nt".into(), 0),
                ("#a".into(), Some("op".into()), "+o-v lee bob".into(), 2),
            ]
        );
        assert_eq!(invite, "bob invited you to #secret");
    }

    // ==================== 접속 정보 / MOTD ====================

    /// 접속 정보 numeric은 서버 버퍼 메시지로, MOTD는 376까지 모아 메시지 하나로 보낸다.
    #[tokio::test]
    async fn info_numerics_and_collected_motd_go_to_server_buffer() {
        let port = spawn_join_reply_mock(concat!(
            ":mock 002 nick :Your host is mock\r\n",
            ":mock 252 nick 3 :operator(s) online\r\n",
            ":mock 375 nick :- mock Message of the Day -\r\n",
            ":mock 372 nick :- line one\r\n",
            ":mock 372 nick :- line two\r\n",
            ":mock 376 nick :End of /MOTD command.\r\n",
        ))
        .await;
        let mut h = NickHarness::new(port);

        h.connect();
        wait_for_status(&mut h.rx, h.server_id, ServerStatus::Connected).await;
        // 001 직후의 "서버에 연결되었습니다." 안내는 건너뛴다
        assert_eq!(
            next_system_message(&mut h.rx).await,
            "서버에 연결되었습니다."
        );
        h.api.join_channel(h.server_id, "#a").unwrap();

        assert_eq!(next_system_message(&mut h.rx).await, "Your host is mock");
        assert_eq!(next_system_message(&mut h.rx).await, "3 operator(s) online");
        assert_eq!(
            next_system_message(&mut h.rx).await,
            "- mock Message of the Day -\nline one\nline two"
        );
    }

    // ==================== 서버 삭제 ====================

    /// 연결된 서버를 삭제하면 QUIT을 보내 연결을 정리하고, 상태/config.json에서 빠지며
    /// UI에 ServerRemoved를 보낸다. 다음 실행 때 복원(자동 접속)되지 않는다.
    #[tokio::test]
    async fn removing_connected_server_quits_and_persists() {
        let (port, mock) = spawn_mock_server().await;
        let mut h = NickHarness::new(port);

        h.connect();
        wait_for_status(&mut h.rx, h.server_id, ServerStatus::Connected).await;

        h.api.remove_server(h.server_id).unwrap();

        let got_quit = timeout(Duration::from_secs(5), mock)
            .await
            .expect("mock server timed out")
            .unwrap();
        assert!(got_quit, "QUIT was not sent on removal");

        assert!(h.state.get_server(h.server_id).is_none());
        timeout(Duration::from_secs(5), async {
            loop {
                match h.rx.recv().await {
                    Some(UiEvent::ServerRemoved(p)) if p.server_id == h.server_id => return,
                    Some(_) => continue,
                    None => panic!("event channel closed"),
                }
            }
        })
        .await
        .expect("no ServerRemoved event");

        // 저장된 상태에 서버가 남지 않는다 → 재시작해도 다시 접속하지 않는다
        use crate::client::persistence::KhannelStateSnapshot;
        use crate::memento::Memento;
        let snapshot: KhannelStateSnapshot = crate::fs::load(&h.path).unwrap();
        let restored: KhannelState = snapshot.restore();
        assert!(restored.get_all_servers().is_empty());
    }

    /// 없는 서버 삭제는 에러.
    #[tokio::test]
    async fn removing_unknown_server_fails() {
        let (port, _lines) = spawn_taken_nick_mock(&[]).await;
        let h = NickHarness::new(port);
        assert!(h.api.remove_server(uuid::Uuid::now_v7()).is_err());
        assert!(h.state.get_server(h.server_id).is_some());
    }
}
