# khannel 개발 가이드

소스에서 빌드하고 코드를 고치는 사람을 위한 문서입니다. 설치와 사용법은 [README](../README.md)를,
릴리스 절차는 [RELEASING.md](RELEASING.md)를 보세요.

## 기술 스택

| 레이어       | 기술                                                        |
| ------------ | ----------------------------------------------------------- |
| UI           | GPUI (zed-industries/zed, git dependency)                   |
| IRC 코어     | Rust, Tokio (async runtime), irc crate                      |
| 대상 OS      | Linux (X11, Wayland), macOS                                 |
| 코드 품질    | rustfmt, clippy                                             |

## 개발 환경

- [Rust](https://www.rust-lang.org/tools/install) stable 1.98 이상 (GPUI 요구사항)
- Linux 시스템 라이브러리:

  ```bash
  # Fedora
  sudo dnf install -y gcc pkgconf-pkg-config openssl-devel fontconfig-devel freetype-devel \
    wayland-devel libxkbcommon-devel libxkbcommon-x11-devel libX11-devel

  # Debian/Ubuntu
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
릴리스 결과물을 만드는 `release.yml`, 배포 채널로 내보내는 `publish.yml`은 [RELEASING.md](RELEASING.md)에 설명이 있습니다.

### 로그

로그는 표준 출력으로 나가며 기본 레벨은 `info`입니다. `RUST_LOG`로 바꿀 수 있습니다(`tracing-subscriber` EnvFilter).

```bash
RUST_LOG=debug cargo run -p khannel
RUST_LOG=khannel_core=trace,info cargo run -p khannel
```

### 개발 데이터

debug 빌드는 릴리스 데이터와 섞이지 않도록 앱 데이터 디렉토리 아래 `dev/config.json`을 사용합니다
(예: Linux `~/.local/share/com.kimtalmo.khannel/dev/config.json`).

## 아키텍처 개요

```
┌──────────────────────────────────────────────────────┐
│  khannel (GPUI)                                      │
│  ┌─────────────┐  ┌──────────────────────────────┐   │
│  │  IrcModel   │──│ ChatApp / Sidebar /          │   │
│  │  (Entity)   │  │ MessageView / InputBar / 모달 │   │
│  └─────────────┘  └──────────────────────────────┘   │
│        ↑ UiEvent (tokio mpsc 채널)                    │
│        ↓ UiApi → KhannelApi (메서드 호출)              │
├──────────────────────────────────────────────────────┤
│  khannel-core (프레임워크 무관)                         │
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
    ├── remove_server_modal.rs # 서버 삭제 확인
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

## 배포 자산 (`dist/`)

- `app-icon.png` — 아이콘 원본(1536px). `generate-icons.sh`로 아래 아이콘을 다시 만듭니다(ImageMagick 필요).
- `linux/` — 데스크톱 항목(`com.kimtalmo.khannel.desktop`), AppStream metainfo, hicolor 아이콘(32~512px)
- `macos/icons/` — macOS 아이콘 그리드에 맞춰 여백과 옅은 그림자를 둔 iconset PNG(16~1024px). 패키징 때 `.icns`로 변환됩니다.
- `macos/package.sh` — Apple Silicon Mac에서 `khannel.app`과 `khannel-<버전>-macos-arm64.dmg`를 만듭니다.
  cargo-packager가 필요하고(`cargo install cargo-packager --locked`), 설정은 `crates/khannel/Cargo.toml`의
  `[package.metadata.packager]`에 있습니다. 서명은 기본 ad-hoc이며 `macos/sign-app.sh`를 참고하세요.
- `rpm/khannel.spec`, `rpm/make-srpm.sh` — Fedora RPM. `make-srpm.sh`가 현재 커밋과 Linux 대상으로 거른 vendor
  tarball로 SRPM을 만들고(`cargo-vendor-filterer` 필요), RPM 빌드는 오프라인으로 진행됩니다.
  COPR는 `.copr/Makefile`에서 같은 스크립트를 씁니다.
- `homebrew/khannel.rb` — Homebrew Cask 템플릿(`@VERSION@`, `@SHA256@`은 릴리스 게시 때 채워짐)
- `bump-version.sh` — 릴리스 버전 올리기([RELEASING.md](RELEASING.md#버전-올리기) 참고)

## 라이선스 고지

배포 바이너리에 포함되는 의존성의 라이선스 고지는 [THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md)이며
[cargo-about](https://github.com/EmbarkStudios/cargo-about)으로 생성합니다(`about.toml`, `about.hbs`).
의존성이 바뀌면 다시 생성합니다. `dist/bump-version.sh`도 버전을 올릴 때 다시 생성합니다.

```bash
cargo install cargo-about --locked
cargo about generate about.hbs -o THIRD_PARTY_NOTICES.md
```
