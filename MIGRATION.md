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

#### Phase 3 코드 리뷰 → 잔여 항목 (2026-10-27)

Phase 3 종료 시점 코드 리뷰에서 도출. 단순 실수보다는 "Svelte도 원래 그렇다"로
상속된 문제가 많으므로 패리티 회귀가 아니라 **후속 이슈로 분리해 해결**한다.

**버그/UX (Phase 4에서 검증 및 수정)**

1. **모달/컨텍스트 메뉴에 Esc 닫기가 없다.**
   - Svelte `ChangeNicknameModal`은 네이티브 `<dialog>`(`Modal.svelte`)이라 Esc로 닫히면
     `onclose` → `nickErrors` 정리까지 따라갔다. GPUI 모달 3종과 컨텍스트 메뉴에는
     Esc-to-dismiss 바인딩이 전혀 없다 → `key_context` 지정 + Esc 액션 바인딩으로 보완.
2. **`IrcModel::toggle_lock`이 복합 ID(`serverId:channel`)를 그대로 코어에 전달한다.**
   - 코어의 채널 맵(`ServerState::channels`)은 채널명 키 기준이라 `set_channel_locked`가
     항상 no-op이 되고, `emit_channel_lock_changed`는 무조건 발행된다. 즉 UI 배지는 토글돼도
     코어 lock 상태에는 전혀 반영되지 않는다(스냅샷에도 남지 않음 → 세션 한정 상태).
   - 단 원본 Svelte도 동일(`toggleLock` → `channel = ircStore.currentChannelId`)하므로,
     이번 마이그레이션에서는 패리티 우선으로 그대로 둔다. **"코어 채널 키 정규화(lock 채널명)
     이슈"로 Phase 4/5 사이 분리**해서 해결할 것.

**경미한 항목 (nits)**

- `ui/sidebar.rs` 서버 row 선택 배경이 외곽 row에 걸려 채널 리스트 영역까지 덮인다
  (Svelte는 서버 *버튼*에만 배경). 의도한 클러스터 스타일이 아니라면 내부 row로 이동.
- 채널 row hover가 `theme.hover`(neutral-100/800)인데 Svelte 채널 hover는 neutral-200/700 —
  서버 row hover와 톤이 분리돼 있었다. `Theme`에 채널 hover 톤 분리 추가 검토.
- `chat_app.rs` render 안 `window.focus()`: GPUI에서 지양되는 패턴. 포커스 경쟁/플리커가
  보이면 `window.defer(cx, ...)`로 빼기(zed thread_view.rs 참고).
- `ServerModal` 제출 실패(예: "Already connecting") 시에도 모달이 즉시 닫힌다(warn만 기록).
  Svelte는 `await invoke`가 날아가도 모달이 열려 있었다. connect 에러를 모달 에러 영역으로
  표시 + Svelte의 `connecting`/"연결 중..." disabled 상태 복원 여지.
- `Topic` 정리(Topic이 None) 시 `IrcModel`은 `"Topic set to: "` (Svelte는 `"null"`) —
  표시 문자열 정책 통일 필요. 관련: `core.rs` TOPIC emit이 None을 실제 전달하는지 확인.
- 메뉴 등장 `transition:scale` 애니메이션은 드랍됨 (GPUI 자체 스타일 애니메이션 검토 여지).
- clippy 경고 잔량: kirc-app 26 / kirc-core 4 (let_unit_value, HashMap entry API,
  redundant closure 등) — `cargo clippy --fix`로 대부분 해소 가능.
  `impl UiApi` 통째 `#[allow(dead_code)]`는 항목별 허용으로 좁히는 것이 좋다.
- `model.rs` Topic 분기의 `let _ = server;` 불필요 클론 흔적 제거.

### Phase 4 — 수동 패리티 테스트 (진행 중, 2026-10-27)

Tauri 버전과 나란히 실행해 다음을 검증:

1. 서버 연결/해제, 닉네임 중복 처리, 채널 입장/퇴장
2. 메시지 수신/전송(에코 포함), unread 카운트, 잠금(lock)
3. 스냅샷 복원(App data dir에 만들어진 기존 config.json 으로 재시작)
4. 닫기→숨김, 정상 종료(QUIT 전송 후 프로세스 종료)
5. Wayland 집중 점검 + macOS(하이엔드 한글 입력, IME) 2차 검증
6. (선택) 이전 구현이 있던 유닛 테스트는 kirc-core로 이식, 새 앱의 통합 테스트는
   gpui 제공 TestAppContext 유틸(있다면)로.
7. (Phase 3 잔여 항목) "Phase 3 코드 리뷰 → 잔여 항목" 중 패리티 영향 항목 재점검 —
   모달/메뉴 Esc 동작, lock 토글이 GPUI/Tauri 양쪽에서 동일하게 (작동하지 않는) 동작인지 대조.

**진행 내역 (2026-10-27):**

- **모달/컨텍스트 메뉴 Esc 닫기 구현** (Phase 3 리뷰 #1, 패리티 영향 항목):
  - `action.rs`에 `kirc::Dismiss` 액션 신설, `ChatApp::show`에서 `escape` 전역 바인딩.
  - 모달: `modal_overlay`가 `on_dismiss` 리스너를 받도록 변경 — 모달 내부 입력은 포커스
    경로 안에 있으므로 overlay div의 `.on_action`으로 `cancel()` 처리(Svelte `<dialog>`의
    onclose → nickErrors 정리까지 동일 경로로 상속). `dismissed`된 액션은 버블 단계에서
    전파를 멈추므로 아래 전역 리스너와 충돌하지 않는다.
  - 컨텍스트 메뉴: 메뉴는 포커스 경로 밖(sibling) 요소라 `.on_action`으로 받을 수 없다 →
    `App::on_action`(전역 버블 리스너)으로 ChatApp의 `close_menus` 처리.
  - 디스패치 근거: gpui(6b985a8) `key_dispatch.rs`/`window.rs` — 액션은 포커스 노드를
    기점으로 ancestor 체인만 버블링하며, `deferred`는 `defer_draw` 시점에 `parent_node`를
    캡처해 서브트리의 dispatch 조상 관계를 유지한다.
- **clippy 경고 정리** (Phase 3 nit): kirc-app 26 → 0 (`let_unit_value`, HashMap entry API,
  redundant closure, contains_key 등). kirc-core 4 → 0. 남는 1건(`error` field never
  read)은 `src-tauri` 복사본 쪽으로 Phase 5 삭제 대상이라 그대로 둠.
- **검증**: `cargo test --workspace` 25개 통과, Wayland 스모크 런(기존 config.json 복원 +
  서버 액터 기동 + 런 루프 안정 진입) 확인.
- **패리티 테스트에서 발견·수정 (2026-10-27, 4b09e25/4d0469b):**
  1. MessageView 높이 붕괴 — gpui `div` 기본 display는 Block이라 `.flex_col()` 단독으로는
     flex 배치가 안 됐음(`flex_1` 무시). 모든 컬럼 컨테이너에 `.flex()` 추가
     (chat main / sidebar / 서버 row / 모달 폼 body) + MessageView 루트 내부
     `list.flex_1()` 컨테이너에도 `.flex().flex_col()` 적용.
  2. 모달 뒤쪽 클릭 통과 — gpui는 겹친 서브트리에 마우스 이벤트를 z-order 순으로 전부
     발행하므로 리스너 없는 스크림은 클릭을 뒤로 통과시켰음. 스크림에 down/up
     `stop_propagation` 리스너 추가 (패널 자식은 먼저 받으므로 영향 없음).
  3. 잠금 채널에서 IME 입력 주입됨 — 키 액션에는 `enabled` 가드가 있었으나 플랫폼 IME가
     직접 호출하는 `replace_text_in_range`/`replace_and_mark_text_in_range`에는 없었음.
     두 핸들러에 가드 추가.
  - **교훈**: gpui에서 `flex_col`/`flex_row`는 display 설정이 따로 필요하다.
    스타일 포팅 시 `.flex()` 누락은 증상("크기가 0처럼 보임")과 원인이 멀어 추적 비용이 크다.
  - **교훈**: disabled 처리는 키 액션뿐 아니라 `EntityInputHandler`(IME) 경로까지 막아야 한다.
- **테마 교체 (d303cd0)**: 팔레트를 One Light / One Dark로 교체. 반투명 규칙 —
  배경/서페이스 CC(80%), 겹치는 인터랙션 표면(hover/선택/메뉴 hover bg/스크림) 80(50%),
  border/텍스트/작은 요소(badge·상태 점·focus ring)는 불투명. 사용자 확정.

**잔여 — 수동 체크리스트 (GUI 조작 필요, 위 1~7):**
   1~5번 시나리오(서버 연결/해제, 닉 중복, 채널 입퇴장, 메시지/에코, unread, lock,
   스냅샷 복원, 창 닫기/QUIT 종료)를 Tauri 버전과 나란히 대조해 수동 검증.
   단 lock 토글 코어 반영 이슈(Phase 3 리뷰 #2)는 패리티 우선으로 뒀다가 이후 정규화로
   해결했다(3f379c0 — `toggle_lock`이 복합 ID가 아닌 채널명을 코어로 전달하고,
   `ChannelLockChanged` 이벤트를 다시 복합 ID로 조합해 반영; lock은 이제 채널
   스냅샷에 영속된다). Wayland 집중 점검 + macOS IME(한글 입력)는 macOS 장비에서
   2차 검증.
**macOS 1차 검증 결과 (2026-10-27, ccdbecb에서 수정):**
   - macOS에서 텍스트가 전혀 렌더되지 않음 → `gpui_platform`에 `font-kit` feature
     추가(gpui_macos 전용 feature, linux 빌드 영향 없음)로 해결.
   - 반투명 팔레트가 네이티브 불투명 배경에 가려짐 → `WindowOptions.window_background`
     를 `Transparent`로 설정.
   - 잠금 버튼 이모지가 중앙 정렬되지 않음 → 버튼 flex 가운데 정렬.
   - macOS IME(한글 입력) 상세 검증은 잔여 — Wayland 스모크 반복으로 대체 중.

#### 전체 코드 리뷰 → 잔여 항목 (2026-10-28)

완료 표시와 무관하게 `crates/kirc-core` + `crates/kirc-app` 전체를 재검토한 결과.
근거는 lock된 zed `6b985a8`(gpui) / `irc 1.1.0` 소스 대조. 실행 재현이 아닌 코드 판독 기반.
1·2번은 Phase 4 패리티 결과를 왜곡하므로 수동 검증 재개 전에 수정한다.

**높음 (실제 오동작)**

1. **QUIT가 서버로 flush되지 않음** (`core.rs` `run_session`의 Quit 분기).
   `send_quit` 직후 루프를 빠져나와 `ClientStream`이 drop된다. irc 1.1.0은 소켓 쓰기를
   담당하는 `Outgoing`을 `ClientStream::poll_next` 안에서만 구동하므로
   (`irc/src/client/mod.rs` `poll_next`) 큐잉된 QUIT가 쓰이지 않은 채 연결이 끊긴다.
   Disconnect·앱 종료 모두 해당 → 패리티 #4 실패 원인. 수정: QUIT 후 짧은 timeout 동안
   stream을 None/Err까지 계속 poll.
   - **해결 (2026-10-28)**: `drain_after_quit` — QUIT 후 최대 2s(`QUIT_DRAIN_TIMEOUT`,
     `graceful_shutdown`의 5s보다 짧게) stream을 poll. 회귀 테스트
     `core::quit_tests::quit_is_flushed_to_server_on_disconnect`(로컬 mock IRC 서버가 QUIT
     줄 수신 여부 확인)는 수정을 빼면 실패, 넣으면 통과함을 확인.
   - 앱 종료 시 UI 캡(600ms)을 넘겨도 QUIT 쓰기는 drain 첫 poll에서 일어나므로 영향 없음.
2. **연결 상태 전이 경쟁 조건** (`manager.rs` `run_server`, `state/server.rs` `transition_*`).
   액터를 spawn한 **뒤** `transition_to_connecting`을 호출하고, `transition_*`은
   `mem::take` 후 상태 불일치 시 원래 값을 복원하지 않는다(→ Disconnected, tx/handle 유실).
   빠른 연결 실패(예: localhost connection refused) 시 `fail()`이 기록한 Failed를
   `Connecting{종료된 handle}`이 덮어써 `is_active()`가 영구 true → 재연결이
   "Already connecting"으로 거부된다. 수정: spawn 전에 Connecting 확정 + 불일치 시 복원.
   - 1번 테스트 작성 중 재현: Connecting을 거치지 않고 액터를 돌리면 `transition_to_registering`이
     tx를 버려 커맨드 채널이 즉시 닫히고 세션이 Registering 직후 종료된다(불일치 시 값 유실 확인).
   - **해결 (2026-10-28)**: `ServerState::start_connecting(spawn)` — Disconnected/Failed일 때만
     runtime 락 안에서 액터를 spawn하고 Connecting으로 전이(spawn↔전이 원자화,
     `transition_to_connecting` 제거). `transition_to_registering`/`_connected`는 불일치 시
     현재 상태를 복원. 부수 효과로 **Disconnecting 중 재연결 거부** — 이전엔 허용되어 옛 액터의
     종료 전이(`transition_to_disconnected`)가 새 세션 상태를 덮어쓸 수 있었다.
     `connect_server`의 `is_active` 사전 검사(TOCTOU)는 제거하고 `run_server`가 판정.
   - 테스트: `state::server::tests`에 4건(유휴 상태에서만 시작 / Disconnecting 중 거부 /
     불일치 전이 시 상태 유지 / 즉시 실패하는 액터가 Failed로 끝남),
     `manager::tests::fast_connect_failure_is_not_clobbered_by_connecting`(실패 후 재연결
     200회 스모크). 실제 네트워크 경로로는 타이밍상 재현되지 않아 경쟁은 상태 머신 단위로 검증.
3. **컨텍스트 메뉴 클릭 통과 + 항목 선택 후 안 닫힘** (`chat_app.rs` 메뉴 렌더, `context_menu.rs`).
   `occlude()`가 없으면 gpui는 아래 hitbox도 hovered로 판정(`window.rs` `Hitbox::is_hovered`)
   → "Disconnect" 등 클릭 시 아래 서버/채널 row의 `on_click`도 실행되고 hover도 켜진다.
   닫힘은 `on_mouse_down_out`뿐이라 Change Nickname 외 항목은 클릭 후에도 메뉴가 남는다.
   수정: 메뉴 컨테이너 `.occlude()` + 각 항목에서 `close_menus`. 모달 스크림(`modal.rs`)도
   stop_propagation 6개를 `.occlude()`로 교체(뒤쪽 hover/스크롤 휠까지 차단).
   - **수정 (2026-10-28)**: `context_menu_container`와 `modal_overlay` 스크림에 `.occlude()`
     (스크림의 stop_propagation 리스너 6개 제거). 메뉴 항목 7종은 클릭 시 `close_menus` 후
     액션 실행(Change Nickname은 `open_nick_modal`이 닫음). 빌드/clippy/테스트 통과,
     GUI 동작(아래 row 미발화·hover 차단·항목 클릭 후 닫힘·모달 뒤 스크롤 차단) 수동 확인 완료.
4. **접속 시 닉 중복이면 Registering에서 정지** (`core.rs` `NoUsableNick` 분기).
   `alt_nicks: vec![]`이라 등록 단계 433/432는 `NoUsableNick`이 되는데, 코어는 이를
   "닉변 실패"로만 처리하고 계속 진행 → 등록이 끝나지 않고, 에러는 `nick_errors`에만
   있어 닉변 모달을 열어야 보인다. 수정: Registering 상태에서는 사유와 함께 `fail()`.
   - **해결 (2026-10-28)**: 상태가 Registering일 때 `NoUsableNick`이면 `SessionOutcome::Failed(사유)`로
     세션 종료 → `run()`이 Disconnected 대신 `fail()`(Failed + `last_error`) 처리. 등록 이후의
     닉변 실패는 기존대로 `ChangeNickFailed` emit 후 계속. 회귀 테스트
     `core::quit_tests::nick_in_use_during_registration_fails_server`(mock 서버가 NICK에 433 응답)는
     수정 전 Registering 정지로 타임아웃, 수정 후 통과.
   - 잔여: UI는 Failed를 Disconnected와 같은 회색 점으로 표시하고 `last_error`를 어디에도
     보여주지 않는다 → 사유 표시는 10번(서버 메시지 미표시)과 함께 처리.
5. **사이드바 서버/채널 순서 무작위** (`sidebar.rs` — `HashMap::values()` 순회).
   SvelteMap은 삽입 순서 유지 → 패리티 회귀. 수정: `IndexMap` 또는 정렬.
   - **해결 (2026-10-28, 사용자 결정: 삽입 순서 대신 이름순 정렬)**: `IrcModel::sorted_servers`
     (서버 이름 → ID), `sorted_channels(server_id)`(채널명). 모두 대소문자 무시 비교 후 원래
     이름으로 tie-break(IRC 채널명은 대소문자 무시). 사이드바는 렌더 시 정렬 목록을 사용하고,
     전체 채널 맵 clone 대신 현재 서버 채널만 clone한다. 미사용이 된 `IrcModel::channels()` 제거.
     정렬 규칙 단위 테스트 2건(`model::tests`).

**중간**

6. **lock/join/part가 즉시 영속되지 않음** (`commands.rs` lock/unlock, `manager.rs` join/part).
   `save_snapshot` 호출이 없어 정상 종료 시(`prepare_shutdown`)에만 저장 → 크래시/kill 시 유실.
7. **채널 전환 시 스크롤 복원이 자동 스크롤을 멈춤** (`message_view.rs` `sync_list`).
   lastRead == 마지막 메시지(흔한 경우)여도 `pause_following_tail()` → 수동으로 맨 아래로
   내리기 전까지 새 메시지를 따라가지 않는다. 마지막 index면 Tail 유지.
8. **MessageView 렌더 비용 O(n)** (`message_view.rs` render) — 렌더마다 메시지 `Vec` 전체
   clone, `sync_list`가 렌더 중 모델 update. `Arc` 기반 저장/entity 참조로 개선.
9. **긴 입력이 Send 버튼 위로 넘침** (`input.rs` prepaint, `input_bar.rs` 입력 상자) —
   TextInput에 가로 스크롤/clip 없음, 커서가 화면 밖으로 나감.
10. **서버 메시지 미표시** (`model.rs` `add_server_message`) — `server_messages`를 렌더하는
    곳이 없어 "서버에 연결되었습니다."/IRC `ERROR`가 보이지 않는다(Svelte 동일, 4번의 근본 원인).
11. **ChangeNickModal 즉시 닫힘 가능** (`model.rs` Nick 분기, `change_nick_modal.rs` `open_for`) —
    모달 밖에서 생긴 자기 닉 변경도 `nick_success`에 쌓이고, `open_for`가 지우지 않아
    모달을 연 직후 첫 notify에 닫힌다.

**낮음 / Svelte 상속 / nit**

- 닉 비교 대소문자 구분 (`core.rs` NICK, `model.rs` Join/Part/Nick) — IRC 닉은 대소문자 무시.
- 나에게 온 DM이 target(내 닉) 이름 채널에 쌓이고 답장 불가 (`model.rs` UserMessage, 원본 동일).
- 붙여넣기 시 `\n`만 치환 (`input.rs` paste) — `\r`에서 irc 코덱이 잘라 서버엔 첫 줄만,
  로컬 에코는 전체 표시.
- `is_shutting_down`이 Terminated를 막지 않음 (`manager.rs`) — 종료 후 `add_server` 시
  drain된 빈 맵으로 config.json 덮어쓰기 가능(이론상).
- ChannelJoinModal도 실패 시 피드백 없이 닫힘(미연결 서버) — ServerModal 항목과 동일 계열.
- Phase 3 잔여 그대로: render 내 `window.focus()` (`chat_app.rs`), `impl UiApi` 통째
  `#[allow(dead_code)]`, 모달/InputBar의 미사용 `api` 필드.
- ChatApp이 자식 뷰를 observe → 매번 트리 전체 재렌더 (`chat_app.rs`).
- 문서: Phase 2 예제 코드 `application().run(|cx| {` 중복, 오타("배파", "민감 민감", "ox련").

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
