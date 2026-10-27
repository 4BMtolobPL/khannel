//! ChatApp: GPUI 루트 화면. Svelte +page.svelte의 포팅.
//!
//! 좌측 사이드바 (서버/채널 리스트) + 우측 메인 (메시지 뷰 + 입력 영역).
//! 모달/컨텍스트 메뉴는 Phase 3에서 추가.

use std::sync::Arc;

use gpui::{
    div, prelude::*, px, rgb, App, ClickEvent, Context, FocusHandle, Focusable, FontWeight,
    IntoElement, KeyDownEvent, ParentElement, Render, Rgba, Styled,
};
use kirc_core::kirc::types::ChannelId;

use crate::api::{UiApi, UiEventReceiver};
use crate::model::{IrcModel, MessageType, UiServerStatus};

// ==================================== 색상 팔레트 ====================================

#[derive(Clone, Copy)]
struct Palette {
    bg_main: Rgba,
    bg_side: Rgba,
    border: Rgba,
    text_main: Rgba,
    text_subtle: Rgba,
    hover: Rgba,
    accent: Rgba,
}

// 라이트 모드 팔레트 (다크 모드는 Phase 3+)
fn light() -> Palette {
    Palette {
        bg_main: rgb(0xf5f5f5),
        bg_side: rgb(0xfafafa),
        border: rgb(0xd4d4d4),
        text_main: rgb(0x171717),
        text_subtle: rgb(0xa3a3a3),
        hover: rgb(0xe5e5e5),
        accent: rgb(0x0284c7),
    }
}

// ==================================== ChatApp ====================================

pub struct ChatApp {
    model: gpui::Entity<IrcModel>,
    focus: FocusHandle,
    text_buffer: String,
}

impl ChatApp {
    pub fn show(api: Arc<UiApi>, rx: UiEventReceiver, cx: &mut Context<Self>) -> Self {
        let model = cx.new(|inner| IrcModel::start(api, rx, inner));
        cx.observe(&model, |_, _, cx| cx.notify()).detach();

        ChatApp {
            model,
            focus: cx.focus_handle(),
            text_buffer: String::new(),
        }
    }

    fn send_message(&mut self, cx: &mut App) {
        let text = std::mem::take(&mut self.text_buffer);
        let _ = self.model.update(cx, |model, cx| model.send_message(&text, cx));
    }

    fn toggle_lock(&mut self, cx: &mut App) {
        let _ = self.model.update(cx, |model, cx| model.toggle_lock(cx));
    }
}

impl Focusable for ChatApp {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn status_dot_color(is_connected: bool, is_connecting: bool) -> Rgba {
    if is_connected {
        rgb(0x22c55e) // green-500
    } else if is_connecting {
        rgb(0xeab308) // yellow-500
    } else {
        rgb(0xef4444) // red-500
    }
}

impl Render for ChatApp {
    fn render(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 입력창 자동 포커스
        if !self.focus.is_focused(window) {
            window.focus(&self.focus, cx);
        }

        let model = self.model.read(cx);
        let is_locked = model.is_locked();
        let servers = model.servers().clone();
        let unread = model.server_unread();
        let channels = model.channels().clone();
        let current_server_id = model.current_server_id().copied();
        let current_channel_id = model.current_channel_id().map(|s| s.to_string());

        let server = current_server_id
            .and_then(|id| model.servers().get(&id).cloned());
        let messages = model.current_messages().cloned().unwrap_or_default();

        // 서버 선택 전이면 로딩 화면
        let Some(server) = server else {
            return div()
                .flex()
                .size_full()
                .justify_center()
                .items_center()
                .bg(light().bg_main)
                .text_color(light().text_main)
                .child("Select or add a server");
        };

        div()
            .w_full()
            .h_full()
            .flex()
            .bg(light().bg_main)
            .text_color(light().text_main)
            // ── 좌측 사이드바 ──
            .child(
                div()
                    .w(px(160.))
                    
                    .flex_col()
                    .bg(light().bg_side)
                    .border_r_1()
                    .border_color(light().border)
                    .child(
                        div()
                            .px(px(8.))
                            .py(px(8.))
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Servers"),
                    )
                    .child(
                        div().flex_col().gap(px(2.)).px(px(4.)).children(
                            servers.values().map(|item| {
                                let is_selected = item.id == server.id;
                                let unread = unread.get(&item.id).copied().unwrap_or(0);

                                let mut row = div().id(item.id.to_string()).w_full().flex()
                                    .items_center().justify_between().px(px(8.)).py(px(4.))
                                    .cursor_pointer()
                                    .when(is_selected, |d| d.bg(light().hover))
                                    .hover(|h| h.bg(light().hover))
                                    .on_click({
                                        let id = item.id;
                                        let model = self.model.clone();
                                        move |_, _, cx| {
                                            let _ = model.update(cx, |m, cx| {
                                                m.select_server(id, cx);
                                            });
                                        }
                                    })
                                    .child(
                                        div().overflow_x_hidden().child(item.name.clone()),
                                    );

                                if !is_selected && unread > 0 {
                                    row = row.child(
                                        div().px(px(4.)).py(px(0.5)).text_xs()
                                            .bg(rgb(0xef4444)).text_color(rgb(0xffffff))
                                            .rounded_full()
                                            .child(if unread > 99 {
                                                "99+".to_string()
                                            } else {
                                                unread.to_string()
                                            }),
                                    );
                                }

                                row.child(
                                    div().size(px(8.)).rounded_full().bg(status_dot_color(
                                        item.status == UiServerStatus::Connected,
                                        item.status == UiServerStatus::Connecting
                                            || item.status == UiServerStatus::Registering,
                                    )),
                                )
                            }),
                        ),
                    )
                    // 채널 목록 (현재 서버)
                    .child(
                        div().flex_col().gap(px(2.)).ml(px(12.)).children(
                            channels
                                .values()
                                .filter(|c| current_server_id == Some(c.server_id))
                                .map(|item| {
                                    // ircStore와 동일하게 current_channel_id는
                                    // 복합 ID("serverId:channel")를 유지한다.
                                    let channel_id =
                                        model.get_channel_id(&item.server_id, &item.name);
                                    let is_selected =
                                        current_channel_id.as_ref() == Some(&channel_id);
                                    let mut row = div()
                                        .id(channel_id.clone())
                                        .w_full()
                                        .px(px(6.))
                                        .py(px(3.))
                                        .cursor_pointer()
                                        .when(is_selected, |d| d.bg(light().hover))
                                        .hover(|h| h.bg(light().hover))
                                        .on_click({
                                            let model = self.model.clone();
                                            move |_, _, cx| {
                                                let _ = model.update(cx, |m, cx| {
                                                    m.set_current_channel(Some(channel_id.clone()), cx);
                                                });
                                            }
                                        })
                                        .child(item.name.clone());

                                    if item.unread > 0 {
                                        row = row.child(
                                            div().px(px(3.)).text_xs().bg(rgb(0xef4444))
                                                .text_color(rgb(0xffffff)).rounded_full()
                                                .child(item.unread.min(99).to_string()),
                                        );
                                    }

                                    row
                                }),
                        ),
                    ),
            )
            // ── 우측 메인 ──
            .child(
                div().flex_1().flex_col().overflow_hidden()
                    // 메시지 목록
                    .child(
                        div().flex_1().p(px(10.)).flex_col().gap(px(4.))
                            .children(messages.iter().map(|message| match message.kind {
                                MessageType::User => div()
                                    .flex()
                                    .gap(px(8.))
                                    .child(
                                        div().font_weight(FontWeight::BOLD)
                                            .text_color(light().accent)
                                            .child(message.nickname.clone().unwrap_or_default()),
                                    )
                                    .child(message.content.clone()),
                                MessageType::System => div()
                                    .text_sm()
                                    .text_color(light().text_subtle)
                                    .child(message.content.clone()),
                            })),
                    )
                    // 입력 영역
                    .child(
                        div().flex().gap(px(8.)).p(px(10.)).border_t_1().border_color(light().border)
                            .child(button("lock", if is_locked { "🔒" } else { "🔓" }).on_click(
                                cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_lock(cx)),
                            ))
                            .child(
                                div().flex_1().rounded(px(4.)).px(px(8.)).py(px(6.)).text_sm()
                                    .bg(rgb(0xffffff)).border_1().border_color(light().border)
                                    .when(is_locked, |d| d.opacity(0.5))
                                    .when(!self.text_buffer.is_empty(), |text| text.child(self.text_buffer.clone()))
                                    .when(self.text_buffer.is_empty(), |text| {
                                        text.text_color(light().text_subtle)
                                            .child("Type a message")
                                    })
                                    .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                                        handle_composer_key(this, event, cx);
                                    })),
                            )
                            .child(button("send", "Send").when(is_locked, |d| d.opacity(0.4)).on_click(
                                cx.listener(|this, _: &ClickEvent, _, cx| this.send_message(cx)),
                            )),
                    ),
            )
    }
}

fn handle_composer_key(this: &mut ChatApp, event: &KeyDownEvent, cx: &mut Context<ChatApp>) {
    let key = event.keystroke.key.as_str();
    match key {
        "enter" => this.send_message(cx),
        "backspace" => {
            this.text_buffer.pop();
            cx.stop_propagation();
        }
        "space" => {
            this.text_buffer.push(' ');
            cx.stop_propagation();
        }
        single if single.chars().count() == 1
            && !event.keystroke.modifiers.control
            && !event.keystroke.modifiers.alt =>
        {
            this.text_buffer.push_str(single);
            cx.stop_propagation();
        }
        _ => {}
    }
}

// ==================================== 헬퍼 ====================================

fn button(id: &'static str, label: &str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .cursor_pointer()
        .rounded(px(4.))
        .px(px(8.))
        .py(px(4.))
        .text_sm()
        .hover(|h| h.bg(light().hover))
        .child(label.to_string())
}

// Phase 3 확장용 유틸
#[allow(dead_code)]
fn unread_badge(unread: u32) -> String {
    if unread > 99 {
        "99+".into()
    } else {
        unread.to_string()
    }
}

#[allow(dead_code)]
fn channel_divider(channel: &ChannelId) -> String {
    format!("channel: {}", channel)
}
