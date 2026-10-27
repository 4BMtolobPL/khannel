//! kirc-app: GPUI 기반 UI.
//!
//! Phase 0 스캐폴딩: GPUI 헬로월드 (빌드/런타임 검증용).
//! Phase 2에서 tokio 브리지와 IrcModel/ChatApp 엔티티로 교체된다.

use gpui::{
    div, prelude::*, px, rgb, size, App, Application, Bounds, Context, SharedString, Window,
    WindowBounds, WindowOptions,
};
use gpui_platform::application;

struct HelloWorld {
    text: SharedString,
}

impl Render for HelloWorld {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .bg(rgb(0x2E7D32))
            .size_full()
            .justify_center()
            .items_center()
            .text_xl()
            .text_color(rgb(0xFFFFFF))
            .child(format!("Hello, {}! (GPUI)", &self.text))
    }
}

fn main() {
    // 최신 gpui 업스트림: Application::new() 제거 → gpui_platform::application() 사용
    let app: Application = application();

    app.run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(500.0), px(500.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_window, cx| cx.new(|_| HelloWorld { text: "World".into() }),
        )
        .unwrap();
    });
}
