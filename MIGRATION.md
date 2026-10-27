# kirc-rs: Tauri → GPUI 마이그레이션 가이드

> 작성일: 2026-10-27
> 대상: `src-tauri`(Rust 네이티브) + `src`(Svelte 5 웹뷰) 구성의 kirc-rs IRC 클라이언트를
> 순수 Rust UI 프레임워크인 **GPUI**(zed-industries/zed) 기반으로 전환

## 0. 확정 사항 (결정 이력)

| 항목 | 결정 |
|---|---|
| 대상 OS | **Linux (X11, Wayland) + macOS** |
| GPUI 소스 | **git dependency**: `https://github.com/zed-industries/zed.git` |
| 트레이 | 필요하나 **유예** — Phase 4 이후 별도 구현 (tray-icon/ksni 등 후보) |
| 전환 전략 | 코어 분리(UI 이벤트 버스 도입) → 병행 빌드(Strangler) → 웹뷰/tauri 제거 |

---

## 1. 현재 아키텍처 요약

```
[웹뷰: Svelte 5 (src/)]                        [네이티브: Rust (src-tauri/)]
  ircService.ts   ←invoke→  commands.rs ←── KircManager ←── ServerActor(tokio, 서버당 1개)
  listen(events)            emits.rs    ←── AppHandle.emit(...)      ↑
  ircStore (SvelteMap)                                   persistence(config.json)
```

핵심 통찰: **Tauri 의존은 대부분 코어 외곽에 격리되어 있다.**

- IRC 코어(`core.rs`, `kirc/state/*`, `manager.rs`, `persistence.rs`, `ctcp.rs`, `types/*`)는
  `AppHandle`만 주입받고, 웹 기술과는 무관한 순수 Rust 로직임.
- Tauri를 직접 참조하는 지점(전환 대상, 5~6개 파일):
  - `lib.rs` — 윈도우/트레이/메뉴/명령 등록 (→ GPUI Application 런처로 교체)
  - `emits.rs` — `tauri::Emitter` (→ UiEvent 버스로 교체)
  - `commands.rs` — `#[tauri::command]` 데코레이터 (→ 일반 메서드로 교체, 로직 유지)
  - `manager.rs`, `core.rs` — `AppHandle` 필드/파라미터 타입 (→ `Arc<dyn UiSink>`)
  - `fs.rs` — `app.path().app_data_dir()` (→ `directories` crate)
- 재작성 대상 (UI 로직 전부): `src/` 아래 Svelte/TS 약 770줄
  - `ircService.ts` (이벤트 → 스토어 머지) — Rust로 포팅, 로직 거의 1:1 이전
  - `ircStore` (SvelteMap) — GPUI Entity 필드로 이관
  - 6개 Svelte 컴포넌트

---

## 2. 목표 구조

```
Cargo.toml                  # 워크스페이스 루트 (신규)
crates/
  kirc-core/                # IRC 코어 (프레임워크 무관)
    ├── actor/              #   ServerActor (tokio) — 기존 core.rs
    ├── state/              #   ServerState, KircState, snapshots — 기존 state/*
    ├── event.rs            #   UiEvent enum + UiSink trait + mpsc 구현 (신규)
    └── persistence/        #   memento 기반 config.json 유지
  kirc-app/                 # GPUI 앱
    ├── main.rs             #   Application 런처 + tokio 브리지
    ├── model/              #   IrcModel Entity (ircService.ts 포팅)
    ├── ui/                 #   ChatApp, Sidebar, MessageView, InputBar
    ├── overlay/            #   Modal, ContextMenu
    └── action.rs           #   GPUI Actions
src-tauri/ , src/           # 전환 기간 동안 유지 → 패리티 확인 후 삭제 (Phase 5)
```

### GPUI 의존성 선언

```toml
[dependencies]
gpui = { git = "https://github.com/zed-industries/zed.git" }
# 앱 부트스트랩(Platform 제공)은 gpui_platform 크레이트가 담당.
# 대상 OS(Linux X11/Wayland + macOS)에 맞춰 linux 백엔드 feature를 켠다.
gpui_platform = { git = "https://github.com/zed-industries/zed.git", features = ["wayland", "x11"] }
```

- 업스트림 main 추적 시 주의: `Application::new()`는 최신 업스트림에서 **제거**됨.
  앱 생성은 `use gpui_platform::application;` → `application().run(|cx| ...)` 패턴 사용.
- pre-1.0이므로 **breaking change 빈번** → lock 파일을 커밋으로 고정하고,
  업스트림 업데이트는 별도 PR로 분리해 회귀를 명확히 격리.
- Linux 빌드에 필요한 시스템 패키지(libx11, libwayland, fontconfig 등)는 zed 저장소
  build instructions 기준 문서화.

---

## 3. 단계별 실행 계획

### Phase 0 — 워크스페이스 분리 (안전장치)

1. 루트 `Cargo.toml` 워크스페이스 생성, `crates/kirc-core` / `crates/kirc-app` 스캐폴딩.
2. `src-tauri` 로직을 `kirc-core`로 **복사(이동 아님)** — 기존 Tauri 앱은 계속 빌드/실행 가능.
3. `cargo run -p kirc-app` (헬로월드) + `tauri dev` 양쪽이 동작하는 상태에서 커밋.

> 이 병행 가능성이 최대 안전장치. 문제 생기면 언제든 Tauri 앱으로 복귀 가능.

### Phase 1 — 코어의 UiSink 추상화 (완료, 2026-10-27)

**Phase 1 완료.** Tauri 의존을 코어에서 제거. **기능 변화 0, 검증 가능한 리팩터링 커밋.**

**결과: `kirc-core`는 tauri 의존이 완전히 제거됨.** `cargo test -p kirc-core` 14개 통과.

달성 내역:
- `kirc/emits.rs` 삭제 → `kirc/event.rs` 신설:
  - `UiEvent` enum (7종 emit_* 이벤트 통합)
  - `UiSink` trait + `EventSender = Arc<dyn UiSink>`, `Arc<T: UiSink + ?Sized>` 블랭킷 구현,
    `tokio::sync::mpsc::UnboundedSender<UiEvent>` 기본 구현
  - 토큰-프리(`CollectingSink`)와 채널(`ui_sink_via_channel`) 유닛 테스트 포함
- `manager.rs`, `core.rs`의 `app_handle: AppHandle` → `ui_sink: EventSender`
  (기존 `emit_*` 호출부 시그니처만 교체, emit은 이제 fire-and-forget, 오류는 메서드 내 로깅)
- `commands.rs`: `#[tauri::command]` 제거 → `KircApi` facade(플레인 메서드)로 포트.
  `manager.kirc_state()` accessor 추가.
- 가시성: 코어 API 전반을 `pub`으로 공개 (kirc-app에서 사용하기 위한 전 단계)

참고: 계획서 상의 "Tauri 어댑터"는 불필요해졌다. 기존 Tauri 앱(src-tauri)은
이제 코어의 **복사본**을 계속 소유하며(`src-tauri/src`) kirc-core를 참조하지 않기 때문이다.

### Phase 2 — GPUI 코어 뼈대 + tokio 브리지 (완료, 2026-10-27)

**결과: GPUI 앱이 Wayland에서 기동되어 런 루프에 안정 진입.** `cargo test --workspace` 25개 통과.

구현 내역 (`crates/kirc-app`):
- `main.rs`:
  - `logging::init_logging()` — tauri 의존 없이 동일 유지
  - app data dir: `directories::BaseDirs` data dir + Tauri identifier(`com.kimtalmo.kirc-rs`) —
    구 Tauri 앱이 쓰던 config.json을 그대로 공유 (Phase 4 패리티 #3 목적), debug 빌드에 `/dev` suffix 유지
  - 스냅샷 복원 → `KircManager` → `KircApi` 세트 구성
  - 종료 흐름: `Quit` 액션(Ctrl+Q / Cmd+Q)과 마지막 창 닫기 모두
    `UiApi::shutdown_blocking(600ms 캡)` → `cx.quit()` — `action.rs` 참고
  - `gpui_platform::application()`으로 오픈 윈도우 760x520
- `api.rs` (tokio ↔ GPUI 브리지):
  - 독립 tokio multi-thread 런타임 소유, ServerActor는 `rt.spawn`
  - 모든 UI 명령은 `rt.enter()` 가드 후 kirc-core `KircApi` 호출
  - 안전 종료: `shutdown_blocking(max_wait)` — 스냅샷 저장은 drain 이전에 동기 완료되므로
    캡 초과 시 QUIT 완료 대기만 잘리고 상태 유실은 없음. 재진입(Quit 액션 후 cx.quit → on_app_quit)은 skip.
  - **교훈**: GPUI executor 퓨처 안에서는 `tokio::time::sleep` 금지(리액터 없음)
    → 대기는 `cx.background_executor().timer(...)` 사용
  - `UiEvent` 채널 (tx→rx) 로 core 이벤트를 UI로 전달
- `model.rs`: `IrcModel` Entity — `ircStore` 필드(servers/channels/messages/currents/nick_errors) 이식
  + `ircService.ts`의 kirc:event switch(UserMessage/Join/Part/Quit/Nick/Topic/Error) 1:1 이식
  + store operations/queries(ensure_channel, add_message, server_unread 등) 이식
  + `currentChannelId`도 원본과 동일하게 **복합 ID**(`serverId:channel`)을 유지해야 함 —
    사이드바 채널 클릭 시 반드시 `get_channel_id`로 조합해 저장 (옛 Svelte 구현과의 차이 포인트)
- `chat_app.rs`: `ChatApp` 루트 뷰 — 사이드바(서버 rows: 이름/unread 배지/status dot),
  채널 리스트(현재 서버), 메시지 목록(User/System), 입력창(키 네이밍 기반 간이 composer), Send 버튼.
  `cx.observe(model)` 로 렌더 갱신.
- 한글/IME 입력은 Phase 3에서 정식 input 컴포넌트로 교체.
```rust
// crates/kirc-app/main.rs
fn main() {
    logging::init_logging();

    // 스냅샷 로드 — fs.rs는 tauri path API 대신 directories crate 사용
    // (app_data_dir + "/dev" 레이아웃 개념은 debug 빌드에서 유지)
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<UiEvent>();
    let sink = Arc::new(tx);           // impl UiSink for UnboundedSender 래퍼
    // ... KircState 복원, Manager 생성(rt, sink) ...

    // 최신 업스트림에서는 Application::new()가 제거되어 gpui_platform::application() 사용.
    application().run(|cx| {
    // 최신 업스트림에서는 Application::new()가 제거되어 gpui_platform::application() 사용.
    application().run(|cx| {        // 1) IrcModel Entity: rx 이벤트 소비 → 상태 반영 (= ircService.ts 포팅)
        // 2) ChatApp Entity: render — 사이드바/메시지뷰/입력창
    });
}
```

- **tokio ↔ GPUI**: `tokio::sync` 프리미티브는 토큰 런타임 없이 await 가능 →
  GPUI `cx.spawn(async move |cx, _| { while let Some(ev) = rx.recv().await { ... } })`
  로 UI 스레드 소비. IRC 액터는 `rt.spawn(actor.run())` (manager.rs 그대로).
- **명령 처리**: `commands.rs`의 데코레이터만 벗기고 내용은 그대로 manager/state 메서드로.
  UI(액션, 이벤트 핸들러)에서 직접 호출. 비동기 커맨드(connect 등)는 GPUI 쪽 `cx.spawn` + `rt` 조합.
- **액션 종료 흐름**: Quit(메뉴/단축키) → `manager.shutdown()` → `cx.quit()`.
  트레이는 유예 단계이므로 지금은 창의 닫기 버튼에도 안전 종료 흐름 임시 연결.

### Phase 3 — 화면 포팅 (컴포넌트 단위) (완료, 2026-10-27)

`svelte → GPUI div` 1:1 매핑. 모달/입력 등은 `gpui-component` ecosystem 활용 검토.
순서 권장: 입력창 + 서버리스트 → 메시지 뷰 → 모달 → 컨텍스트 메뉴.

**결과: Svelte UI 6종(사이드바/메시지뷰/입력바 + 모달 3종)과 컨텍스트 메뉴 2종을 GPUI로 1:1 포팅.
`cargo test --workspace` 25개 통과, Wayland 스모크 실행(기존 config.json 복원 + IRC 세션 기동) 확인.**

계획 대비 결정 사항:
- `gpui-component` ecosystem **미사용** — crates.io 배포판은 이 프로젝트의 zed git dep와
  버전이 어긋나 공유할 수 없어, 모달/입력/메뉴 모두 자체 구현 (`overlay/`, `ui/input.rs`).
- 입력창: gpui `examples/input.rs`의 정석 구현을 포팅한 `ui/input.rs` `TextInput` 엔티티
  (`EntityInputHandler` + `window.handle_input` 등록 → 플랫폼 IME(X11 XIM / Wayland
  text-input)가 엔티티로 텍스트를 주입). Enter 제출은 조합 중(`marked_range` 존재)이 아닐
  때만 발화 — 한글 IME 안전. 표준 편집 액션(Backspace/Delete/방향키/SelectAll/Copy/Cut/
  Paste/ShowCharacterPalette)은 `action.rs`에 정의하고 `ChatApp::show`에서 바인딩.
- 메시지 뷰: GPUI `list` 가상화 + `FollowMode::Tail` (Svelte `isBottom` 자동 스크롤 대응).
  채널 전환 시 `pause_following_tail()` + `scroll_to(ListOffset)`로 lastReadMessageId
  위치 복원(Svelte `scrollIntoView({block:"start"})` 동일). 스크롤 핸들러는 ListState의
  RefCell이 빌려진 상태에서 호출되므로 반드시 `cx.defer`로 빼서 모델을 갱신한다
  (zed `thread_view.rs` 패턴).
- 컨텍스트 메뉴: `deferred(anchored().position(pos).snap_to_window_with_margin(...))` +
  `on_mouse_down_out` 닫기 — Svelte의 window click listener 대체. 채널 row 우클릭은
  `cx.stop_propagation()`으로 서버 row로의 버블링을 막는다(원본 Svelte에서 두 메뉴가
  동시에 뜨던 버그 수정).
- 다크 모드: `Theme::current(cx)`가 `cx.window_appearance()`로 라이트/다크 팔레트 선택
  (Tailwind `dark:` 변형 대응). `rgb()`/`rgba()`는 const가 아니므로 팔레트는 함수 로컬
  바인딩으로 구성.
- 모달 3종은 영속 엔티티로 `ChatApp`이 생성·보유하고 bool 플래그로 표시/숨긴다.
  각 모달의 `DismissEvent` 구독이 플래그를 해제하며, composer 자동 포커스는 모달이
  열려 있지 않을 때만 시도한다.
- Svelte 개선 포인트: Connect/Copy 메뉴 항목은 원본에서 핸들러 없는 데코레이션이었으나
  실제 액션(connect/copy-to-clipboard)에 연결. 메뉴 hover도 원본 Tailwind 클래스 3종
  (Neutral/Accent/Danger)을 `MenuItemStyle`로 그대로 매핑.

구현 파일 (`crates/kirc-app/src`):
- `ui/` — `chat_app.rs`(루트 조율 + 컨텍스트 메뉴 state), `sidebar.rs`, `message_view.rs`,
  `input_bar.rs`, `input.rs`(IME TextInput)
- `overlay/` — `modal.rs`(공용 레이아웃/스크림), `server_modal.rs`, `channel_join_modal.rs`,
  `change_nick_modal.rs`, `context_menu.rs`
- `theme.rs`(팔레트), `action.rs`(편집 액션),
  `model.rs`(add_server/join_channel/nick_error/nick_success 등 UI 액션 추가)

| Svelte | GPUI |
|---|---|
| `+page.svelte` 사이드바 + Main | `ChatApp::render` — 서버/채널 리스트, 메시지 리스트, 입력폼 |
| 컨텍스트 메뉴 ×2 | 자체 deferred+anchored 메뉴 (`overlay/context_menu.rs`; 좌표, 노출/숨김 플래그는 기존 state 동일) |
| `ServerModal`, `ChannelJoinModal`, `ChangeNicknameModal` | 자체 오버레이 (`overlay/modal.rs` + 모달 3종) |
| `MessageView` 자동 스크롤/UnreadDivider | `list` + `FollowMode::Tail` + lastReadMessageId 로직 동일 이식 |
| Tailwind class | GPUI 스타일 (dark mode: `cx.window_appearance()` 기반 `Theme::current`) |
| 상태: SvelteMap/`$state`/`$derived`/`$effect` | Entity 필드 + `cx.observe` + render 타임 계산 |

이벤트 → 상태 반영 로직(`ircService.ts`의 switch 문)은 `IrcModel` 메서드로 이관:
UserMessage/Join/Part/Quit/Nick/Topic/Error 각 케이스 그대로, 단
Svelte 반응성 대신 "변경사항 → `cx.notify()`"로 변경.

### Phase 4 — 수동 패리티 테스트

Tauri 버전과 나란히 실행해 다음을 검증:

1. 서버 연결/해제, 닉네임 중복 처리, 채널 입장/퇴장
2. 메시지 수신/전송(에코 포함), unread 카운트, 잠금(lock)
3. 스냅샷 복원(App data dir에 만들어진 기존 config.json 으로 재시작)
4. 닫기→숨김, 정상 종료(QUIT 전송 후 프로세스 종료)
5. Wayland 집중 점검 + macOS(하이엔드 한글 입력, IME) 2차 검증
6. (선택) 이전 구현이 있던 유닛 테스트는 kirc-core로 이식, 새 앱의 통합 테스트는
   gpui 제공 TestAppContext 유틸(있다면)로.

### Phase 5 — 잔여 항목 및 정리

- **트레이 구현** (별도 이슈): zed 프레임워크는 미제공 — `tray-icon`/`ksni` 별도 스레드.
  존재하지 않는 기능(Show/Quit 메뉴)을 fork에 붙이는 것은 비권장(유지보수 리스크).
- **네이티브 메뉴**: zed 프레임워크는 macOS 메뉴 지원. Linux는 상황 확인 후 대응.
- 제거: `src/`, `src-tauri/`, `svelte.config.js`, `vite.config.js`, `tsconfig.json`,
  `deno.lock`, `package.json` (svelte/ox련 도구들), README/CI 정합성 수정.
- 수동: "IRC client built with Tauri" → "IRC client built with GPUI".

---

## 4. 리스크 & 완화

| 리스크 | 완화 |
|---|---|
| GPUI pre-1.0, 버전 간 breaking change | git dep + lock 고정, 업데이트를 별도 PR로 격리 |
| Linux X11/Wayland 성숙도(특히 Wayland) | Phase 2 첫 주에 헬로월드 + Phase 4에서 Wayland 집중 테스트 |
| macOS 민감 민감(IME/한글 입력, 메뉴) | Phase 4 체크리스트 포함, 문제시 최대한 표준 컴포넌트 사용 |
| 스크롤/렌더 성능(대량 메시지) | GPUI list 기반 가상화, 필요 시 표시 범위 렌더만 |
| 트레이/윈도우 숨김 등 네이티브 UX 축소 | 트레이는 유예로 명시, 개별 세부는 사용자 확인 |
| 유지보수성(알 수 없는 패턴) | 화면을 작은 컴포넌트로 쪼개고, Phase별 커밋으로 격리 |
| 이전 기능 회귀 | Phase 0~4 동안 Tauri 앱 유지, 패리티 체크리스트 완료 후 삭제 |

---

## 5. 참고 자료

- GPUI 홈: https://gpui.rs/ (docs, examples)
- 업스트림: https://github.com/zed-industries/zed.git (gpui crate; App/Entity/Render 패턴)
- Zed 실사용 코드(최대 참고 유용): zed repo `crates/`
- crates에 공식 배파 버전(gpui 0.2.x, macOS/Linux)도 존재하지만, 결정상 git dep 사용.
