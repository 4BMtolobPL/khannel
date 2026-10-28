//! Sidebar: 서버/채널 리스트 + 추가 버튼 (+ 우클릭 컨텍스트 메뉴 요청).
//!
//! Svelte `+page.svelte`의 `<aside>`를 1:1 포팅. 선택 상태는 IrcModel이 소유하고,
//! 모달/컨텍스트 메뉴 열기는 이벤트로 상위(ChatApp)에 위임한다 — Svelte에서
//! `+page.svelte`가 직접 들고 있던 `showServerModal`/`showChannelModal`/컨텍스트 메뉴
//! state에 해당한다.

use gpui::{
    div, prelude::*, px, App, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    IntoElement, MouseButton, MouseDownEvent, ParentElement, Pixels, Point, Render, Styled,
    TextAlign, Window,
};
use kirc_core::kirc::types::ServerId;

use crate::model::{Channel, IrcModel, Server, UiServerStatus};
use crate::theme::{status_dot_color, Theme};

// ==================================== 이벤트 ====================================

#[derive(Debug)]
pub enum SidebarEvent {
    OpenServerModal,
    OpenChannelModal,
    ServerContextMenu {
        server_id: ServerId,
        /// 윈도우 좌표 (Svelte의 clientX/clientY)
        position: Point<Pixels>,
    },
    ChannelContextMenu {
        server_id: ServerId,
        channel_id: String,
        channel_name: String,
        position: Point<Pixels>,
    },
}

impl EventEmitter<SidebarEvent> for Sidebar {}

// ==================================== 뷰 ====================================

pub struct Sidebar {
    model: Entity<IrcModel>,
    focus: FocusHandle,
}

impl Sidebar {
    pub fn new(model: Entity<IrcModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        Self {
            model,
            focus: cx.focus_handle(),
        }
    }
}

impl Focusable for Sidebar {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// unread 배지. 서버 row는 `min-w-5 px-2`, 채널 row는 `px-1.5` (Svelte와 동일).
fn badge(unread: u32, theme: Theme, small: bool) -> gpui::Div {
    div()
        .when(small, |d| d.px(px(6.)))
        .when(!small, |d| d.px(px(8.)))
        .py(px(2.))
        .min_w(px(20.))
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .bg(theme.badge_bg)
        .text_color(theme.badge_text)
        .rounded_full()
        .text_align(TextAlign::Center)
        .child(if unread > 99 {
            "99+".to_string()
        } else {
            unread.to_string()
        })
}

impl Render for Sidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::current(cx);
        // HashMap 순회 순서는 삽입마다 바뀌므로 표시 순서는 정렬된 목록으로 고정한다.
        let servers: Vec<Server> = self
            .model
            .read(cx)
            .sorted_servers()
            .into_iter()
            .cloned()
            .collect();
        let server_unread = self.model.read(cx).server_unread();
        let current_server_id = self.model.read(cx).current_server_id().copied();
        let current_channel_id = self
            .model
            .read(cx)
            .current_channel_id()
            .map(|s| s.to_string());

        div()
            .w(px(224.)) // Svelte w-56
            .flex_shrink_0()
            // ChatApp이 [위쪽 여백 + 사이드바] 세로 컨테이너로 감싸므로 남은 높이를 채운다
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(theme.bg_side)
            .border_r_1()
            .border_color(theme.border)
            .child(
                div()
                    .p(px(8.))
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Servers"),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_y(px(4.)) // space-y-1
                    .px(px(8.))
                    .children(servers.iter().map(|item| {
                        let is_selected = Some(item.id) == current_server_id;
                        let unread = server_unread.get(&item.id).copied().unwrap_or(0);
                        let server_id = item.id;
                        let server_name = item.name.clone();
                        let is_connected = item.status == UiServerStatus::Connected;
                        let is_connecting = item.status == UiServerStatus::Connecting
                            || item.status == UiServerStatus::Registering;

                        let mut row = div()
                            .id(server_id.to_string())
                            .w_full()
                            .flex()
                            .flex_col()
                            .rounded(px(4.))
                            .when(is_selected, |d| d.bg(theme.selected))
                            .child(
                                // ── 서버 row ──
                                div()
                                    .id(format!("server-row-{}", server_id))
                                    .w_full()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .px(px(12.))
                                    .py(px(8.))
                                    .cursor_pointer()
                                    .when(!is_selected, |d| d.hover(|h| h.bg(theme.hover)))
                                    .on_click({
                                        let model = self.model.clone();
                                        move |_, _, cx| {
                                            model
                                                .update(cx, |m, cx| m.select_server(server_id, cx));
                                        }
                                    })
                                    .on_mouse_down(
                                        MouseButton::Right,
                                        cx.listener(move |_, event: &MouseDownEvent, _, cx| {
                                            cx.emit(SidebarEvent::ServerContextMenu {
                                                server_id,
                                                position: event.position,
                                            });
                                        }),
                                    )
                                    .child(
                                        // Left: 서버 이름 (truncate)
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .overflow_x_hidden()
                                            .whitespace_nowrap()
                                            .child(server_name),
                                    )
                                    .child(
                                        // Right: unread 배지 + 상태 점
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_x(px(8.))
                                            .flex_shrink_0()
                                            .when(!is_selected && unread > 0, |d| {
                                                d.child(badge(unread, theme, false))
                                            })
                                            .child(
                                                div()
                                                    .size(px(8.)) // h-2 w-2
                                                    .rounded_full()
                                                    .bg(status_dot_color(
                                                        &theme,
                                                        is_connected,
                                                        is_connecting,
                                                    )),
                                            ),
                                    ),
                            );

                        // 현재 서버의 채널 리스트 (Svelte: li 내부에 렌더)
                        if is_selected {
                            row = row.child(self.render_channel_list(
                                server_id,
                                &current_channel_id,
                                theme,
                                cx,
                            ));
                        }

                        row
                    }))
                    .child(
                        // + 서버 추가
                        div()
                            .id("add-server")
                            .w_full()
                            .mt(px(8.))
                            .rounded(px(4.))
                            .px(px(8.))
                            .py(px(4.))
                            .text_sm()
                            .cursor_pointer()
                            .text_color(theme.text_subtle) // light: neutral-600 대체, dark: neutral-400
                            .hover(|h| h.bg(theme.hover))
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(SidebarEvent::OpenServerModal);
                            }))
                            .child("+ 서버 추가"),
                    ),
            )
    }
}

impl Sidebar {
    fn render_channel_list(
        &self,
        server_id: ServerId,
        current_channel_id: &Option<String>,
        theme: Theme,
        cx: &Context<Self>,
    ) -> gpui::Div {
        let model = self.model.clone();
        let channels: Vec<Channel> = model
            .read(cx)
            .sorted_channels(server_id)
            .into_iter()
            .cloned()
            .collect();

        let list = div()
            .flex()
            .flex_col()
            .gap_y(px(4.))
            .ml(px(16.))
            .mt(px(4.))
            .text_sm()
            .children(channels.iter().map(|item| {
                // 리스너 클로저는 'static이므로 참조 대신 값을 복사해 둔다.
                let item_server_id = item.server_id;
                let channel_name = item.name.clone();
                let unread = item.unread;
                // ircStore와 동일하게 current_channel_id는 복합 ID("serverId:channel")
                let channel_id = model
                    .read(cx)
                    .get_channel_id(&item_server_id, &channel_name);
                let is_selected = current_channel_id.as_ref() == Some(&channel_id);
                // move 클로저가 channel_name을 소비하지 않도록 리스너용 복제본을 둔다.
                let listener_channel_name = channel_name.clone();

                div()
                    .id(channel_id.clone())
                    .w_full()
                    .rounded(px(4.))
                    .px(px(8.))
                    .py(px(4.))
                    .cursor_pointer()
                    .when(is_selected, |d| d.bg(theme.selected_channel))
                    .when(!is_selected, |d| d.hover(|h| h.bg(theme.hover)))
                    .on_click({
                        let model = model.clone();
                        let channel_id = channel_id.clone();
                        move |_, _, cx| {
                            model.update(cx, |m, cx| {
                                m.set_current_channel(Some(channel_id.clone()), cx)
                            });
                        }
                    })
                    // 채널 우클릭: 서버 row로의 이벤트 버블링을 막아
                    // 두 메뉴가 동시에 뜨는 Svelte 버그를 방지한다.
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |_, event: &MouseDownEvent, _, cx| {
                            cx.emit(SidebarEvent::ChannelContextMenu {
                                server_id: item_server_id,
                                channel_id: channel_id.clone(),
                                channel_name: listener_channel_name.clone(),
                                position: event.position,
                            });
                            cx.stop_propagation();
                        }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_x(px(4.))
                            .child(channel_name.clone())
                            .when(unread > 0, |d| d.child(badge(unread, theme, true))),
                    )
            }));

        // + 채널 추가 (Svelte: 채널 리스트 마지막 항목)
        list.child(
            div()
                .id("add-channel")
                .w_full()
                .rounded(px(4.))
                .px(px(8.))
                .py(px(4.))
                .cursor_pointer()
                .text_color(theme.text_subtle)
                .hover(|h| h.bg(theme.hover))
                .on_click(cx.listener(|_, _, _, cx| {
                    cx.emit(SidebarEvent::OpenChannelModal);
                }))
                .child("+ 채널 추가"),
        )
    }
}
