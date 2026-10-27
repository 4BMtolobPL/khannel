//! kirc-app: GPUI 기반 IRC 클라이언트.
//!
//! Phase 2: 코어 뼈대(경로/로깅/런처) + tokio 브리지 + IrcModel/ChatApp 엔티티.

mod action;
mod api;
mod chat_app;
mod model;

use std::sync::Arc;
use std::time::Duration;

use gpui::{App, Bounds, KeyBinding, Size, WindowBounds, WindowOptions, prelude::*, px};
use gpui_platform::application;
use kirc_core::fs;
use kirc_core::kirc::persistence::KircStateSnapshot;
use kirc_core::logging;
use kirc_core::memento::Memento;

use crate::action::Quit;
use crate::api::{UiApi, build_kirc, setup};
use crate::chat_app::ChatApp;

/// Graceful shutdown에 허용하는 최대 대기 시간.
///
/// - `manager.shutdown()`은 서버별로 QUIT 전송 후 actor 종료를 최대 5s까지 기다리지만,
///   창 닫기/단축키 종료는 UI를 곧바로 닫아버리는 흐름이라 앱을 장시간 막으면 안 된다.
/// - 스냅샷 저장은 drain 이전에 동기로 완료되므로, 캡을 넘어도 상태 유실은 없다.
const SHUTDOWN_MAX_WAIT: Duration = Duration::from_millis(600);

fn app_data_dir() -> std::path::PathBuf {
    // Tauri app_data_dir(XDG data dir / identifier)과 동일 경로를 유지해
    // 기존 Tauri 앱이 만든 config.json을 그대로 읽고 쓴다 (Phase 4 패리티 #3).
    // - Linux:  ~/.local/share/com.kimtalmo.kirc-rs
    // - macOS:  ~/Library/Application Support/com.kimtalmo.kirc-rs
    let mut base = directories::BaseDirs::new()
        .map(|d| d.data_dir().join("com.kimtalmo.kirc-rs"))
        .unwrap_or_else(|| std::env::home_dir().unwrap().join(".kirc-rs"));

    // Tauri 앱의 app_data_dir + "/dev" 레이아웃 개념은 debug 빌드에서 유지
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
    let snapshot: KircStateSnapshot =
        fs::load(&config_path).expect("snapshot load failed");

    let mut state = snapshot.restore();
    state.set_persistence_path(&config_path);

    let kirc_api = build_kirc(ui_sink.clone(), state);
    let api = Arc::new(UiApi::new(rt, kirc_api, ui_sink));

    // 2. GPUI 앱 실행
    application().run(move |cx: &mut App| {
        // ── 안전 종료 흐름 (MIGRATION.md Phase 2: "Quit → manager.shutdown() → cx.quit()") ──
        // 1) 단축키(Ctrl+Q / Cmd+Q): Quit 액션 →  shutdown → quit
        let quit_api = api.clone();
        cx.bind_keys([
            KeyBinding::new("ctrl-q", Quit, None),
            KeyBinding::new("cmd-q", Quit, None),
        ]);
        cx.on_action::<Quit>(move |_, cx| {
            tracing::info!("Quit action received");
            quit_api.shutdown_blocking(SHUTDOWN_MAX_WAIT);
            cx.quit();
        });

        // 2) 창의 닫기 버튼: Linux에서는 마지막 창이 닫히면 곧바로 앱 종료가 되므로
        //    on_app_quit에서 동일한 graceful shutdown을 거친다 (이미 시작했으면 재진입 스킵).
        let quit_api = api.clone();
        cx.on_app_quit(move |_cx| {
            tracing::info!("App quit requested");
            quit_api.shutdown_blocking(SHUTDOWN_MAX_WAIT);
            std::future::ready(())
        })
        .detach();

        let bounds = Bounds::centered(None, Size::new(px(760.), px(520.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            move |_window, cx| cx.new(|cx| ChatApp::show(api.clone(), rx, cx)),
        )
        .expect("open main window failed");
    });
}
