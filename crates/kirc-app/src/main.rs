//! kirc-app: GPUI 기반 IRC 클라이언트.
//!
//! Phase 2: 코어 뼈대(경로/로깅/런처) + tokio 브리지 + IrcModel/ChatApp 엔티티.

mod api;
mod chat_app;
mod model;

use std::sync::Arc;

use gpui::{App, Bounds, Context, Size, Window, WindowBounds, WindowOptions, div, prelude::*, px, rgb};
use gpui_platform::application;
use kirc_core::fs;
use kirc_core::kirc::persistence::KircStateSnapshot;
use kirc_core::logging;
use kirc_core::memento::Memento;

use crate::api::{UiApi, build_kirc, setup};
use crate::chat_app::ChatApp;

fn app_data_dir() -> std::path::PathBuf {
    // Tauri app_data_dir 대신 directories crate로 대체 (Linux: ~/.local/share)
    let mut base = directories::ProjectDirs::from("", "", "kirc-rs")
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| std::env::home_dir().unwrap().join(".kirc-rs"));

    if cfg!(debug_assertions) {
        base.push("dev");
    }
    base
}

struct LoadingScreen {
    message: String,
}

impl gpui::Render for LoadingScreen {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .size_full()
            .justify_center()
            .items_center()
            .bg(rgb(0xf5f5f5))
            .text_color(rgb(0x171717))
            .child(self.message.clone())
    }
}

fn main() {
    logging::init_logging();

    // 1. 코어 세팅
    let (rt, ui_sink, rx) = setup();

    // 스냅샷 로드
    let dir = app_data_dir();
    std::fs::create_dir_all(&dir).expect("app data dir create failed");
    let config_path = dir.join("config.json");
    let snapshot: KircStateSnapshot =
        fs::load(&config_path).expect("snapshot load failed");

    let mut state = snapshot.restore();
    state.set_persistence_path(&config_path);

    let kirc_api = build_kirc(rt.clone(), ui_sink.clone(), state);
    let api = Arc::new(UiApi::new(rt, kirc_api, ui_sink));

    // 2. GPUI 앱 실행
    application().run(move |cx: &mut App| {
        let api = api.clone();
        let rx = rx;

        let bounds = Bounds::centered(None, Size::new(px(760.), px(520.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            move |_window, cx| cx.new(|cx| ChatApp::show(api, rx, cx)),
        )
        .expect("open main window failed");
    });
}
