//! 모달 공통 프레임.
//!
//! Svelte 모달들의 `fixed inset-0 z-50 flex items-center justify-center bg-black/30`
//! 오버레이를 GPUI `deferred`로 구현한다. (gpui-component는 crates.io gpui를 쓰므로
//! git dep(zed)과 소스가 달라 타입이 통일되지 않음 → MIGRATION.md의 "자체 오버레이" 선택)

use gpui::{
    deferred, div, prelude::*, px, App, ClickEvent, Div, IntoElement, ParentElement, SharedString,
    Styled, Window,
};

use crate::action::Dismiss;
use crate::theme::Theme;

/// 모달 닫힘 이벤트. 각 모달은 `EventEmitter<DismissEvent>`를 구현하고,
/// 상위(ChatApp)가 이를 받아 모달을 숨긴다.
#[derive(Debug)]
pub struct DismissEvent;

/// 전체 화면 스크림 + 중앙 정렬 패널. `deferred`로 최상단에 렌더링된다.
///
/// `on_dismiss`: 모달 입력이 포커스된 상태에서 `Dismiss`(Esc) 액션이 버블링되면 호출된다.
/// (액션 디스패치는 포커스 경로만 따르므로, 모달 입력 → overlay 체인을 탄다.
/// deferred는 `defer_draw` 시점에 `parent_node`를 캡처하므로 서브트리의
/// dispatch 조상 관계는 유지된다. → Svelte 네이티브 `<dialog>`의 Esc 대체)
pub fn modal_overlay(
    panel: impl IntoElement,
    theme: Theme,
    on_dismiss: impl Fn(&Dismiss, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    deferred(
        div()
            .absolute()
            .size_full()
            .bg(theme.overlay_scrim) // black/30
            .flex()
            .items_center()
            .justify_center()
            .on_action(on_dismiss)
            .child(panel),
    )
}

/// 모달 패널. Svelte: `w-full max-w-{size} rounded-xl bg-white p-6 shadow-lg`
pub fn modal_panel(
    max_width: gpui::Pixels,
    padding: gpui::Pixels,
    theme: Theme,
) -> gpui::Stateful<Div> {
    div()
        .id("modal-panel")
        .w_full()
        .max_w(max_width)
        .rounded(px(12.))
        .bg(theme.panel)
        .p(padding)
        .shadow_lg()
}

/// 모달 헤더: 제목 + ✕ 닫기 버튼 (Svelte ServerModal 헤더와 동일 배치)
pub fn modal_header(
    title: impl Into<SharedString>,
    theme: Theme,
    on_close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .mb(px(16.))
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .text_lg()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(title.into()),
        )
        .child(
            div()
                .id("modal-close")
                .cursor_pointer()
                .text_color(theme.text_subtle)
                .hover(|h| h.text_color(theme.text_main))
                .on_click(on_close)
                .child("✕"),
        )
}

/// 폼 입력 라벨 (Svelte: `block text-sm font-medium mb-1`)
pub fn field_label(label: &str, theme: Theme) -> Div {
    div()
        .text_sm()
        .font_weight(gpui::FontWeight::MEDIUM)
        .mb(px(4.))
        .text_color(theme.text_main)
        .child(label.to_string())
}

/// 에러 문구 (Svelte: `text-sm text-red-500`)
pub fn error_text(text: &str, theme: Theme) -> Div {
    div()
        .text_sm()
        .text_color(theme.error_text)
        .child(text.to_string())
}
