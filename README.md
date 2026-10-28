# khannel

[GPUI](https://gpui.rs/)와 Rust로 만든 현대적인 데스크톱 IRC 클라이언트입니다.

> Tauri + SvelteKit 버전(이전 이름 `kirc-rs`)에서 GPUI로 전환하면서 이름을 `khannel`로 바꿨습니다.
> 전환 과정과 결정 이력은 [MIGRATION.md](MIGRATION.md)를 참고하세요.

## 기술 스택

| 레이어       | 기술                                                        |
| ------------ | ----------------------------------------------------------- |
| UI           | GPUI (zed-industries/zed, git dependency)                   |
| IRC 코어     | Rust, Tokio (async runtime), irc crate                      |
| 대상 OS      | Linux (X11, Wayland), macOS                                 |
| 코드 품질    | rustfmt, clippy                                             |

## 기능

- **다중 서버 연결**: 여러 IRC 서버에 동시 접속 지원, 서버 우클릭 > Delete Server로 서버 삭제(확인 후 저장된 설정까지 제거)
- **채널 관리**: 채널 참가 / 퇴장, 채널별 메시지 분리 (사이드바는 서버·채널 이름순 정렬)
- **DM(쿼리)**: 나에게 온 개인 메시지는 보낸 사람 닉 버퍼에 모이고, 그 버퍼에서 바로 답장
- **채널 잠금**: 채널을 잠가 실수로 메시지를 보내는 것을 방지 (잠금 상태는 즉시 저장)
- **읽지 않은 메시지 관리**: 서버 및 채널별 미확인 메시지 배지, 첫 미확인 메시지 위 구분선(Unread Divider)
- **서버 버퍼**: 서버만 선택하면 연결 알림·IRC ERROR·연결 실패 사유 등 서버 메시지 표시
- **닉네임 변경**: 서버별 닉네임 변경 및 저장(다음 접속에도 사용). 연결이 끊긴 상태에서도 변경 가능
- **닉네임 선점 대응**: 접속 닉이 사용 중이면 `닉_`, `닉__`, `닉___`으로 자동 접속
- **CTCP 프로토콜 지원**: VERSION, PING, TIME 등의 CTCP 요청 자동 파싱 및 응답 처리
- **컨텍스트 메뉴**: 서버 및 채널 우클릭을 통한 관리 (접속/해제, 참가/퇴장, 닉네임 변경 등)
- **IRC 이벤트 처리**: PRIVMSG, JOIN, PART, QUIT, NICK, TOPIC, ERROR 처리
- **에코 메시지**: 내가 보낸 메시지 채팅창 즉시 표시
- **한글 IME 입력**: 플랫폼 IME(X11 XIM / Wayland text-input / macOS) 지원, 조합 중 Enter는 전송하지 않음
- **자동 재연결**: 앱 시작 시 이전에 연결한 서버에 자동 접속
- **상태 영속성**: 서버·채널 설정을 JSON 파일로 저장 및 복원
- **정상 종료**: 종료 시 서버에 QUIT 전송 후 종료
- **다크 모드**: 시스템 외관에 따라 One Light / One Dark 팔레트 자동 선택
- **구조화된 로깅**: `tracing`을 활용한 디버그/런타임 구조화 로깅
- **TLS 지원**: 보안 연결(TLS) 옵션
- **미니멀 창 상단**: 타이틀바 없이 왼쪽 위의 버튼 island에 사이드바 토글만 표시 (macOS는 신호등과 함께, Linux는 우측 상단에 닫기 버튼). zeron UI 참고 — [ZERON_UI.md](ZERON_UI.md)
- **창 닫기 동작**: macOS는 창을 닫아도 앱이 계속 실행되어 메시지를 받고, Dock 아이콘을 클릭하면 창이 다시 열림 (종료: ⌘Q / 앱 메뉴 > 종료 / Dock > 종료). Linux는 창을 닫으면 종료
- **시스템 트레이**: 예정

## 아키텍처 개요

```
┌──────────────────────────────────────────────────────┐
│  khannel (GPUI)                                      │
│  ┌─────────────┐  ┌──────────────────────────────┐   │
│  │  IrcModel   │──│ ChatApp / Sidebar /          │   │
│  │  (Entity)   │  │ MessageView / InputBar / 모달 │   │
│  └─────────────┘  └──────────────────────────────┘   │
│        ↑ UiEvent (tokio mpsc 채널)                    │
│        ↓ UiApi → KhannelApi (메서드 호출)             │
├──────────────────────────────────────────────────────┤
│  khannel-core (프레임워크 무관)                       │
│  ┌──────────────┐   ┌─────────────┐                  │
│  │KhannelManager│──→│ ServerActor │ (Tokio task)     │
│  └──────────────┘   └──────┬──────┘                  │
│  ┌──────────────┐          │ irc crate stream        │
│  │ KhannelState │          ↓                         │
│  │  (Mutex)     │   IRC Server (TCP/TLS)             │
│  └──────────────┘                                    │
└──────────────────────────────────────────────────────┘
```

- IRC 코어는 `UiSink` trait으로만 UI에 이벤트를 보내며 GPUI에 의존하지 않습니다.
- `khannel`은 독립 tokio 런타임을 소유하고(서버 액터 실행), GPUI 쪽에서 `UiEvent`를 소비해
  `IrcModel` 엔티티 상태로 반영합니다.

### 코어 구조 (`crates/khannel-core/src/`)

```
crates/khannel-core/src/
├── lib.rs
├── error.rs            # 공통 에러 타입
├── fs.rs               # JSON 영속성 (load / save)
├── logging.rs          # 구조화된 로깅 (tracing) 초기화
├── memento.rs          # Memento 패턴 trait (Originator / Memento)
└── client/
    ├── commands.rs     # KhannelApi — UI에서 호출하는 명령 facade + payload 타입
    ├── core.rs         # ServerActor (Tokio 비동기 루프 및 IRC 이벤트 처리)
    ├── ctcp.rs         # CTCP 메시지 파싱 및 응답 처리 (VERSION, PING, TIME)
    ├── event.rs        # UiEvent enum + UiSink trait (UI 이벤트 버스)
    ├── manager.rs      # KhannelManager (서버 생명주기 관리)
    ├── persistence.rs  # 스냅샷 직렬화 구조체
    ├── state/
    │   ├── app.rs      # AppState 열거형 (Running / ShuttingDown / Terminated)
    │   ├── channel.rs  # ChannelState
    │   ├── khannel.rs  # KhannelState (서버 맵, 영속성)
    │   └── server.rs   # ServerState + ServerRuntime FSM
    └── types/
        └── server.rs   # ServerConfig  (types.rs: ServerId, ChannelId, ServerStatus, ServerCommand)
```

### 앱 구조 (`crates/khannel/src/`)

```
crates/khannel/src/
├── main.rs             # 앱 진입점: 경로/로깅, 스냅샷 복원, 창 열기, 종료 흐름
├── api.rs              # UiApi — tokio 런타임 소유 + khannel-core 명령 브리지
├── model.rs            # IrcModel — UiEvent → 서버/채널/메시지 상태 반영
├── action.rs           # GPUI 액션 (Quit, Dismiss, 텍스트 편집)
├── theme.rs            # One Light / One Dark 팔레트
├── ui/
│   ├── chat_app.rs     # 루트 뷰 (창 컨트롤 + 사이드바 + 메인 + 모달/컨텍스트 메뉴 조율)
│   ├── sidebar.rs      # 서버/채널 목록
│   ├── message_view.rs # 메시지 목록 (가상화 list, 자동 스크롤, Unread Divider)
│   ├── input_bar.rs    # 잠금 토글 + 메시지 입력 + Send
│   ├── input.rs        # IME 지원 단일 줄 TextInput
│   └── window_frame.rs # Linux client-side decorations (그림자/테두리/가장자리 리사이즈)
└── overlay/
    ├── modal.rs        # 모달 공통 프레임 (스크림/패널)
    ├── server_modal.rs # 서버 연결
    ├── channel_join_modal.rs
    ├── change_nick_modal.rs
    └── context_menu.rs # 서버/채널 우클릭 메뉴
```

### 서버 상태 머신 (ServerRuntime FSM)

```
Disconnected ──connect──→ Connecting ──(취소)──→ Disconnected
                               │ TCP 연결 성공
                               ↓
                          Registering ──RPL_WELCOME──→ Connected
                               │                           │
                     (연결 실패 / 닉네임 거부)          disconnect()
                               ↓                           ↓
                            Failed                   Disconnecting ──→ Disconnected
```

연결 시작은 Disconnected/Failed에서만 가능하며, 액터 spawn과 Connecting 전이는 원자적으로 수행됩니다.

## 개발 환경 설정

### 필수 요구사항

- [Rust](https://www.rust-lang.org/tools/install) (stable)
- Linux 시스템 라이브러리 (Debian/Ubuntu 기준):

  ```bash
  sudo apt install -y pkg-config libssl-dev libfontconfig-dev libfreetype-dev \
    libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev libx11-xcb-dev
  ```

- macOS: Xcode Command Line Tools

### 실행 및 빌드

```bash
# 개발 실행
cargo run -p khannel

# 릴리스 빌드
cargo build -p khannel --release
```

### 테스트 / 품질 검사

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

CI(`.github/workflows/main.yml`)는 push/PR마다 Ubuntu와 macOS에서 fmt/clippy/test를 실행합니다.
릴리스 패키징(.app/.dmg, AppImage 등)은 아직 정해지지 않았습니다.

## 데이터 영속성

서버 설정과 채널 목록(잠금 상태 포함)은 OS 앱 데이터 디렉토리의 `config.json`에 저장됩니다.
서버 추가, 채널 참가/퇴장, 잠금 변경 시 즉시 저장되고 종료 시에도 저장됩니다.

- **macOS**: `~/Library/Application Support/com.kimtalmo.khannel/config.json`
- **Linux**: `~/.local/share/com.kimtalmo.khannel/config.json`

debug 빌드는 같은 디렉토리 아래 `dev/config.json`을 사용합니다.

저장되는 정보: 서버 호스트·포트·TLS·닉네임, 참가 중인 채널 목록 및 잠금 상태.
메시지 히스토리는 세션 내 메모리에만 유지되며 저장되지 않습니다.

## 배포 자산 (`dist/`)

- `app-icon.png` — 아이콘 원본 (1536px). `dist/generate-icons.sh`로 아래 아이콘을 다시 만든다 (ImageMagick 필요).
- `linux/` — 데스크톱 항목(`com.kimtalmo.khannel.desktop`), AppStream metainfo, hicolor 아이콘(32~512px)
- `macos/icons/` — macOS 아이콘 그리드에 맞춰 여백과 옅은 그림자를 둔 iconset PNG(16~1024px, 패키징 시 `.icns`로 변환)
- `macos/package.sh` — Apple Silicon Mac에서 `khannel.app`과 `khannel-<버전>-macos-arm64.dmg`를 만든다
  (cargo-packager 필요: `cargo install cargo-packager --locked`, 설정은 `crates/khannel/Cargo.toml`의
  `[package.metadata.packager]`). 기본은 ad-hoc 서명, `macos/sign-app.sh` 참고
- `rpm/khannel.spec`, `rpm/make-srpm.sh` — Fedora RPM. `make-srpm.sh`가 현재 커밋 + Linux 대상으로 거른 vendor
  tarball로 SRPM을 만들고(`cargo-vendor-filterer` 필요), 빌드는 오프라인. COPR는 `.copr/Makefile`로 같은 스크립트를 쓴다

## 라이선스

[MIT](LICENSE). 배포 바이너리에 포함되는 의존성의 라이선스 고지는 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)에 있다
(의존성이 바뀌면 `cargo about generate about.hbs -o THIRD_PARTY_NOTICES.md`로 다시 생성 — `about.toml` 참고).
