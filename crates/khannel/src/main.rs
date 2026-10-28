//! khannel: GPUI 기반 IRC 클라이언트.
//!
//! Phase 2: 코어 뼈대(경로/로깅/런처) + tokio 브리지 + IrcModel/ChatApp 엔티티.

mod action;
mod api;
mod model;
mod overlay;
mod theme;
mod ui;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    App, Bounds, Entity, Global, KeyBinding, Menu, MenuItem, Size, TitlebarOptions, WindowBounds,
    WindowDecorations, WindowOptions, point, prelude::*, px,
};
use gpui_platform::application;
use khannel_core::client::persistence::KhannelStateSnapshot;
use khannel_core::fs;
use khannel_core::logging;
use khannel_core::memento::Memento;

use crate::action::{
    Backspace, Copy, Cut, Delete, DeleteWordLeft, DeleteWordRight, Dismiss, End, Home, Left, Paste,
    Quit, Right, SelectAll, SelectLeft, SelectRight, SelectWordLeft, SelectWordRight, WordLeft,
    WordRight,
};
use crate::api::{UiApi, build_khannel, setup};
use crate::model::IrcModel;
use crate::ui::chat_app::ChatApp;

/// 앱 수준 IRC 상태. 창과 생명주기를 분리해, macOS에서 창을 닫아도(앱은 계속 실행)
/// 이벤트를 계속 받아 쌓고, 창을 다시 열면 그대로 이어서 보여준다.
struct MainModel(Entity<IrcModel>);

impl Global for MainModel {}

/// 앱 식별자 — 데이터 디렉토리, Wayland app_id 공통.
const APP_ID: &str = "com.kimtalmo.khannel";

/// Graceful shutdown에 허용하는 최대 대기 시간.
///
/// - `manager.shutdown()`은 서버별로 QUIT 전송 후 actor 종료를 최대 5s까지 기다리지만,
///   창 닫기/단축키 종료는 UI를 곧바로 닫아버리는 흐름이라 앱을 장시간 막으면 안 된다.
/// - 스냅샷 저장은 drain 이전에 동기로 완료되므로, 캡을 넘어도 상태 유실은 없다.
const SHUTDOWN_MAX_WAIT: Duration = Duration::from_millis(600);

/// 앱 데이터 폴더 (XDG data dir / identifier)
/// - Linux:  ~/.local/share/com.kimtalmo.khannel
/// - macOS:  ~/Library/Application Support/com.kimtalmo.khannel
fn app_data_dir() -> PathBuf {
    let mut base = directories::BaseDirs::new()
        .map(|d| d.data_dir().join(APP_ID))
        .unwrap_or_else(|| std::env::home_dir().unwrap().join(format!(".{APP_ID}")));

    // debug 빌드는 별도 하위 폴더를 쓴다 (개발 중 데이터가 릴리스 데이터와 섞이지 않게)
    if cfg!(debug_assertions) {
        base.push("dev");
    }
    base
}

fn main() {
    logging::init_logging();

    // 1. 코어 세팅
    let (rt, ui_sink, rx) = setup();

    // 스냅샷 로드 (config.json이 없으면 빈 상태로 시작)
    let dir = app_data_dir();
    std::fs::create_dir_all(&dir).expect("app data dir create failed");
    let config_path = dir.join("config.json");
    let snapshot: KhannelStateSnapshot = fs::load(&config_path).expect("snapshot load failed");

    let mut state = snapshot.restore();
    state.set_persistence_path(&config_path);

    let khannel_api = build_khannel(ui_sink.clone(), state);
    let api = Arc::new(UiApi::new(rt, khannel_api, ui_sink));

    // 2. GPUI 앱 실행
    let app = application();

    // macOS: 창이 모두 닫힌 뒤 Dock 아이콘을 클릭하면 창을 다시 연다.
    // (macOS 기본 종료 정책은 마지막 창을 닫아도 앱을 종료하지 않는다. Linux는 마지막 창을
    //  닫으면 종료하므로 이 경로를 타지 않는다.)
    app.on_reopen(|cx| {
        if cx.windows().is_empty()
            && let Some(model) = cx.try_global::<MainModel>().map(|m| m.0.clone())
        {
            open_main_window(model, cx);
        }
    });

    app.run(move |cx: &mut App| {
        bind_keys(cx);

        // macOS 시스템 메뉴바: 앱 이름 메뉴에 "종료" 하나만 둔다. 창이 모두 닫혀 있으면
        // 키 입력을 받을 창이 없으므로, Cmd+Q가 동작하려면 메뉴 항목이 필요하다.
        // (Linux에서는 gpui가 메뉴를 그리지 않으므로 등록하지 않는다.)
        if cfg!(target_os = "macos") {
            cx.set_menus([Menu {
                name: "khannel".into(),
                items: vec![MenuItem::action("종료", Quit)],
                disabled: false,
            }]);
        }

        // ── 안전 종료 흐름 (docs/MIGRATION.md Phase 2: "Quit → manager.shutdown() → cx.quit()") ──
        // 1) 단축키(Cmd+Q / Ctrl+Q) 또는 macOS 메뉴 "종료": Quit 액션 → shutdown → quit
        let quit_api = api.clone();
        cx.on_action::<Quit>(move |_, cx| {
            tracing::info!("Quit action received");
            quit_api.shutdown_blocking(SHUTDOWN_MAX_WAIT);
            cx.quit();
        });

        // 2) 그 밖의 종료 경로(Linux 마지막 창 닫기, macOS Dock > 종료 등)는 on_app_quit에서
        //    동일한 graceful shutdown을 거친다 (이미 시작했으면 재진입 스킵).
        let quit_api = api.clone();
        cx.on_app_quit(move |_cx| {
            tracing::info!("App quit requested");
            quit_api.shutdown_blocking(SHUTDOWN_MAX_WAIT);
            std::future::ready(())
        })
        .detach();

        // Esc → 컨텍스트 메뉴 닫기. 메뉴는 포커스 경로 밖 요소라 모달처럼 .on_action으로
        // 받을 수 없어, 모달이 Dismiss를 소비하지 못한 경우 전역 버블 리스너로 닫는다.
        cx.on_action::<Dismiss>(|_, cx| {
            for window in cx.windows() {
                if let Some(chat) = window.downcast::<ChatApp>() {
                    chat.update(cx, |this, _, cx| this.close_menus(cx)).ok();
                }
            }
        });

        let model = cx.new(|cx| IrcModel::start(api.clone(), rx, cx));
        cx.set_global(MainModel(model.clone()));
        open_main_window(model, cx);
        // 실행 직후 앱을 포그라운드로 가져온다 (macOS: 터미널/Finder에서 실행해도 창이 뒤에 깔리지 않게)
        cx.activate(true);
    });
}

/// 키 바인딩 (앱 전체에서 1회 — 창을 다시 열 때마다 중복 등록되지 않게).
/// 종료/편집 단축키는 OS 관례에 맞춰 macOS는 cmd, 그 외는 ctrl 조합만 바인딩한다.
fn bind_keys(cx: &mut App) {
    #[cfg(target_os = "macos")]
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        // TextInput 표준 편집 액션 (gpui examples/input.rs 참고)
        KeyBinding::new("cmd-a", SelectAll, None),
        KeyBinding::new("cmd-c", Copy, None),
        KeyBinding::new("cmd-v", Paste, None),
        KeyBinding::new("cmd-x", Cut, None),
        // macOS 관례: cmd-←/→ = 줄 처음/끝
        KeyBinding::new("cmd-left", Home, None),
        KeyBinding::new("cmd-right", End, None),
        // macOS 관례: option(alt) = 단어 단위 이동/선택/삭제
        KeyBinding::new("alt-left", WordLeft, None),
        KeyBinding::new("alt-right", WordRight, None),
        KeyBinding::new("alt-shift-left", SelectWordLeft, None),
        KeyBinding::new("alt-shift-right", SelectWordRight, None),
        KeyBinding::new("alt-backspace", DeleteWordLeft, None),
        KeyBinding::new("alt-delete", DeleteWordRight, None),
        KeyBinding::new("ctrl-cmd-space", crate::action::ShowCharacterPalette, None),
    ]);
    // Linux 등: 종료 단축키는 두지 않는다 — 창 닫기는 컴포지터/WM 단축키나 닫기 버튼으로
    // (사용자 결정). 종료 흐름은 on_app_quit에서 동일하게 graceful shutdown을 거친다.
    #[cfg(not(target_os = "macos"))]
    cx.bind_keys([
        // TextInput 표준 편집 액션 (gpui examples/input.rs 참고)
        KeyBinding::new("ctrl-a", SelectAll, None),
        KeyBinding::new("ctrl-c", Copy, None),
        KeyBinding::new("ctrl-v", Paste, None),
        KeyBinding::new("ctrl-x", Cut, None),
        // ctrl = 단어 단위 이동/선택/삭제
        KeyBinding::new("ctrl-left", WordLeft, None),
        KeyBinding::new("ctrl-right", WordRight, None),
        KeyBinding::new("ctrl-shift-left", SelectWordLeft, None),
        KeyBinding::new("ctrl-shift-right", SelectWordRight, None),
        KeyBinding::new("ctrl-backspace", DeleteWordLeft, None),
        KeyBinding::new("ctrl-delete", DeleteWordRight, None),
    ]);

    // OS 공통 편집 키
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, None),
        KeyBinding::new("delete", Delete, None),
        KeyBinding::new("left", Left, None),
        KeyBinding::new("right", Right, None),
        KeyBinding::new("shift-left", SelectLeft, None),
        KeyBinding::new("shift-right", SelectRight, None),
        KeyBinding::new("home", Home, None),
        KeyBinding::new("end", End, None),
    ]);
    // Esc → 모달/컨텍스트 메뉴 닫기 (khannel::Dismiss)
    cx.bind_keys([KeyBinding::new("escape", Dismiss, None)]);
}

/// 메인 창을 연다 (최초 실행 + macOS Dock 클릭 재오픈 공통).
fn open_main_window(model: Entity<IrcModel>, cx: &mut App) {
    let bounds = Bounds::centered(None, Size::new(px(760.), px(520.)), cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            // 시스템 타이틀바를 숨기고 앱이 상단 바를 직접 그린다 (ui::title_bar).
            // - macOS: 내용이 창 맨 위까지 차고, 신호등 버튼만 그 위에 얹힌다.
            // - Linux: client-side decorations — 닫기 버튼/창 이동/리사이즈를 앱이 처리한다.
            titlebar: Some(TitlebarOptions {
                title: Some("khannel".into()),
                appears_transparent: true,
                // zeron: 신호등(높이 14) 중심 y=21 = 38px 컨트롤 줄 + 위쪽 패딩 4의 중심
                traffic_light_position: Some(point(px(14.), px(14.))),
            }),
            // 앱이 그린 창 이동 영역(WindowControlArea::Drag)을 macOS가 막지 않도록 표시
            // (zeron: AppKit이 영역을 dead-zone 처리하거나 클릭을 지연시키지 않게)
            app_owns_titlebar_drag: true,
            window_decorations: Some(WindowDecorations::Client),
            app_id: Some(APP_ID.to_string()),
            // 반투명 팔레트(One Light/Dark alpha)가 비치려면 네이티브 배경을
            // 투명으로 둬야 한다 (macOS 필수 — 불투명 뒷배경으로 가림).
            // Linux CSD의 그림자 영역도 투명 배경이 필요하다.
            window_background: gpui::WindowBackgroundAppearance::Transparent,
            ..Default::default()
        },
        move |window, cx| cx.new(|cx| ChatApp::show(model, window, cx)),
    )
    .expect("open main window failed");
}
