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
```

- 업스트림 main에 계속 추적하며 필요한 시점에 `Cargo lock` 갱신.
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

### Phase 1 — 코어의 UiSink 추상화 (가장 중요한 단계)

Tauri 의존을 코어에서 제거. **기능 변화 0, 검증 가능한 리팩터링 커밋.**

```rust
// kirc-core/event.rs — 모든 emit을 하나의 enum으로 통합
#[derive(Clone, Debug)]
pub enum UiEvent {
    ServerAdded(ServerDetail),
    ServerStatus  { server_id: ServerId, status: ServerStatus },
    ChannelLockChanged { server_id: ServerId, channel: ChannelId, locked: bool },
    SystemMessage { server_id: ServerId, message: String },
    ChangeNickFailed { server_id: ServerId, reason: String },
    Ui(UIEventPayload),   // 기존 emits.rs의 UIEventPayload 변형 재사용
}

pub trait UiSink: Send + Sync + 'static {
    fn emit(&self, event: UiEvent);
}
```

작업 내역:
- `emits.rs`의 7개 `emit_*`/builder → `UiSink: emit(UiEvent)` 호출로 교체.
  `UIEventBuilder` 패턴은 유지하되 `AppHandle` 대신 `Arc<dyn UiSink>`를 받도록 수정.
- `ServerActor`, `KircManager`는 `AppHandle` 소유 대신 `Arc<dyn UiSink>` 소유.
- Tauri 쪽 어댑터(state.rs 내)를 따로 두어 `AppHandle → UiSink` 구현체 주입
  (기존 Tauri 빌드는 어댑터로 유지).
- `kirc-core`의 **Tauri-프리** 유닛 테스트: std mpsc 기반 UiSink로 emit 검증 (부수 이득).

결과: `kirc-core`에 `tauri` 의존이 사라짐. 커맨드/이벤트 유형은 기존 payload 그대로.

### Phase 2 — GPUI 코어 뼈대 + 경로/로깅/런처

### Phase 2 — GPUI 코어 뼈대 + tokio 브리지

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

    gpui::Application::new().run(|cx| {
        // 1) IrcModel Entity: rx 이벤트 소비 → 상태 반영 (= ircService.ts 포팅)
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

### Phase 3 — 화면 포팅 (컴포넌트 단위)

`svelte → GPUI div` 1:1 매핑. 모달/입력 등은 `gpui-component` ecosystem 활용 검토.
순서 권장: 입력창 + 서버리스트 → 메시지 뷰 → 모달 → 컨텍스트 메뉴.

| Svelte | GPUI |
|---|---|
| `+page.svelte` 사이드바 + Main | `ChatApp::render` — 서버/채널 리스트, 메시지 리스트, 입력폼 |
| 컨텍스트 메뉴 ×2 | GPUI context menu (마우스 이벤트: 좌표, 노출/숨김 플래그는 기존 state 동일) |
| `ServerModal`, `ChannelJoinModal`, `ChangeNicknameModal` | gpui-component Modal 또는 자체 오버레이 |
| `MessageView` 자동 스크롤/UnreadDivider | scrollable + lastReadMessageId 로직 동일 이식 |
| Tailwind class | GPUI 스타일 (dark mode: `ActiveTheme` 환경 or 자체 toggle) |
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
