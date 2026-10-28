//! ContextMenu: 서버/채널 우클릭 메뉴.
//!
//! Svelte `+page.svelte`의 고정 좌표 메뉴 2종을 GPUI `deferred` + `anchored`로 포팅.
//! 좌표/노출 플래그는 기존 state와 동일하게 ChatApp이 소유하고, 외부 클릭 시
//! 닫는 동작은 `on_mouse_down_out`으로 구현한다 (Svelte의 window click listener 대체).

use gpui::{Div, IntoElement, Styled, div, prelude::*, px};

use crate::theme::Theme;

/// 메뉴 컨테이너. Svelte: `min-w-48 bg-white/95 backdrop-blur-md border rounded-lg shadow-xl py-1.5`
pub fn context_menu_container(theme: Theme) -> gpui::Stateful<Div> {
    div()
        .id("context-menu")
        // 메뉴 아래(사이드바 row 등)의 hitbox가 hovered로 판정되지 않게 막는다.
        // gpui는 occlude가 없으면 겹친 하위 hitbox도 hovered로 보므로, 항목 클릭이
        // 아래 row의 on_click까지 발화시키고 hover 스타일도 켜진다.
        .occlude()
        .min_w(px(192.))
        .py(px(6.))
        .rounded(px(8.))
        .border_1()
        .border_color(theme.border)
        .bg(theme.menu_bg)
        .shadow_xl()
        .overflow_hidden()
        .text_color(theme.text_main)
}

/// 메뉴 항목 hover 스타일. Svelte 원본의 hover 클래스 3종에 대응한다.
/// - `Normal`: `hover:bg-neutral-100 dark:hover:bg-neutral-700`
/// - `Accent`: `hover:bg-sky-50 dark:hover:bg-sky-900/30 hover:text-sky-600 dark:hover:text-sky-400`
///   (Connect/Join 항목)
/// - `Danger`: `hover:bg-red-50 dark:hover:bg-red-900/30 hover:text-red-600 dark:hover:text-red-400`
///   (Disconnect/Leave 항목)
#[derive(Clone, Copy)]
pub enum MenuItemStyle {
    Normal,
    Accent,
    Danger,
}

/// 메뉴 항목. Svelte: `w-full text-left px-3 py-1.5 text-sm hover:... flex items-center gap-2`
pub fn context_menu_item(
    id: &'static str,
    icon: &'static str,
    label: &'static str,
    style: MenuItemStyle,
    theme: Theme,
) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .w_full()
        .flex()
        .items_center()
        .gap_x(px(8.))
        .px(px(12.))
        .py(px(6.))
        .text_sm()
        .cursor_pointer()
        .hover(|h| match style {
            MenuItemStyle::Normal => h.bg(theme.menu_hover),
            MenuItemStyle::Accent => h.bg(theme.accent_hover_bg).text_color(theme.accent_text),
            MenuItemStyle::Danger => h.bg(theme.danger_hover_bg).text_color(theme.danger_text),
        })
        .child(
            // Svelte: w-4 text-center opacity-70
            div()
                .w(px(16.))
                .text_align(gpui::TextAlign::Center)
                .opacity(0.7)
                .child(icon),
        )
        .child(label)
}

/// 구분선 (Svelte: `my-1 border-t border-neutral-100`)
pub fn context_menu_separator(theme: Theme) -> Div {
    div()
        .mt(px(4.))
        .mb(px(4.))
        .border_t_1()
        .border_color(theme.border_subtle)
}

/// 컨텍스트 메뉴를 윈도우 좌표(position)에 띄우는 deferred 엘리먼트.
/// `snap_to_window_with_margin`으로 화면 밖으로 나가지 않게 클램프한다.
pub fn anchored_menu(
    position: gpui::Point<gpui::Pixels>,
    menu: gpui::Stateful<Div>,
) -> impl IntoElement {
    gpui::deferred(
        gpui::anchored()
            .position(position)
            .snap_to_window_with_margin(gpui::px(8.))
            .child(menu),
    )
}
