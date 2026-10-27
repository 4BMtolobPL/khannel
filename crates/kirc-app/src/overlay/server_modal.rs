//! ServerModal: 서버 연결 폼 (host/port/TLS/nickname + 검증).
//!
//! Svelte `ServerModal.svelte`의 포팅. 검증은 제출 시 수행하고, 에러는
//! 필드 아래 빨간 텍스트로 표시한다.

use std::sync::Arc;

use gpui::{
    div, prelude::*, px, App, ClickEvent, Context, Entity, EventEmitter, Focusable, IntoElement,
    ParentElement, Render, StatefulInteractiveElement, Styled, Window,
};

use crate::action::Dismiss;
use crate::api::UiApi;
use crate::model::IrcModel;
use crate::overlay::modal::{
    error_text, field_label, modal_header, modal_overlay, modal_panel, DismissEvent,
};
use crate::theme::Theme;
use crate::ui::input::{TextInput, TextInputEvent};

const DEFAULT_PORT: &str = "6697";

pub struct ServerModal {
    #[allow(dead_code)]
    api: Arc<UiApi>,
    model: Entity<IrcModel>,
    host: Entity<TextInput>,
    port: Entity<TextInput>,
    nickname: Entity<TextInput>,
    tls: bool,
    errors: Vec<(&'static str, &'static str)>,
}

impl EventEmitter<DismissEvent> for ServerModal {}

impl ServerModal {
    pub fn new(
        api: Arc<UiApi>,
        model: Entity<IrcModel>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let host = cx.new(TextInput::new);
        let port = cx.new(|cx| TextInput::new(cx).with_text(DEFAULT_PORT));
        let nickname = cx.new(TextInput::new);

        // Enter 제출 (host/nickname 입력창에서) → submit
        for input in [&host, &port, &nickname] {
            cx.subscribe_in(
                input,
                window,
                |this, _input, event: &TextInputEvent, window, cx| {
                    if let TextInputEvent::Submit(_) = event {
                        this.submit(window, cx);
                    }
                },
            )
            .detach();
        }

        cx.observe(&model, |_, _, cx| cx.notify()).detach();

        Self {
            api,
            model,
            host,
            port,
            nickname,
            tls: true,
            errors: Vec::new(),
        }
    }

    /// 모달을 열 때 상태를 초기값으로 되돌린다 (Svelte: 컴포넌트 새로 마운트).
    pub fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.host.update(cx, |input, cx| input.clear(window, cx));
        self.port
            .update(cx, |input, cx| input.set_text(DEFAULT_PORT, window, cx));
        self.nickname
            .update(cx, |input, cx| input.clear(window, cx));
        self.tls = true;
        self.errors.clear();
        cx.notify();
    }

    /// 첫 입력 필드로 포커스를 옮긴다.
    pub fn focus_first(&self, window: &mut Window, cx: &mut App) {
        let focus = self.host.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
    }

    fn set_tls(&mut self, tls: bool, cx: &mut Context<Self>) {
        self.tls = tls;
        cx.notify();
    }

    fn submit(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let host = self.host.read(cx).text().trim().to_string();
        let port_text = self.port.read(cx).text().trim().to_string();
        let nickname = self.nickname.read(cx).text().trim().to_string();

        // Svelte validate()와 동일 규칙
        let mut errors = Vec::new();
        if host.is_empty() {
            errors.push(("host", "서버 주소를 입력하세요"));
        }
        if nickname.is_empty() {
            errors.push(("nickname", "닉네임을 입력하세요"));
        }
        let port = port_text.parse::<u16>().ok();
        if port.is_none() || port == Some(0) {
            errors.push(("port", "유효한 포트를 입력하세요"));
        }
        self.errors = errors;
        if !self.errors.is_empty() {
            cx.notify();
            return;
        }

        self.model.update(cx, |m, cx| {
            m.add_server(host, port.unwrap(), self.tls, nickname, cx)
        });
        self.errors.clear();
        cx.emit(DismissEvent);
    }

    fn cancel(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        // Svelte는 connecting 중이면 cancel_connect를 호출하지만, 실제로는
        // serverId가 세팅되지 않아 도달 불가능한 경로다. 닫기만 수행한다.
        cx.emit(DismissEvent);
    }
}

impl Render for ServerModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::current(cx);

        let host_error = self
            .errors
            .iter()
            .find(|(field, _)| *field == "host")
            .map(|(_, text)| *text);
        let port_error = self
            .errors
            .iter()
            .find(|(field, _)| *field == "port")
            .map(|(_, text)| *text);
        let nick_error = self
            .errors
            .iter()
            .find(|(field, _)| *field == "nickname")
            .map(|(_, text)| *text);

        let panel = modal_panel(px(448.), px(24.), theme) // max-w-md p-6
            .child(modal_header(
                "서버 연결",
                theme,
                cx.listener(|this, _: &ClickEvent, window, cx| this.cancel(window, cx)),
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_y(px(16.)) // space-y-4
                    // Host
                    .child(
                        div().child(field_label("서버 주소", theme)).child(
                            div()
                                .mt(px(4.))
                                .rounded(px(6.))
                                .px(px(12.))
                                .py(px(8.))
                                .text_sm()
                                .bg(theme.bg_input)
                                .border_1()
                                .border_color(if self.host.read(cx).is_focused(window) {
                                    theme.focus_ring
                                } else {
                                    theme.border
                                })
                                .child(self.host.clone()),
                        ),
                    )
                    .children(host_error.map(|text| error_text(text, theme)))
                    // Port + TLS
                    .child(
                        div()
                            .flex()
                            .gap_x(px(12.))
                            .child(
                                div().flex_1().child(field_label("포트", theme)).child(
                                    div()
                                        .mt(px(4.))
                                        .rounded(px(6.))
                                        .px(px(12.))
                                        .py(px(8.))
                                        .text_sm()
                                        .bg(theme.bg_input)
                                        .border_1()
                                        .border_color(if self.port.read(cx).is_focused(window) {
                                            theme.focus_ring
                                        } else {
                                            theme.border
                                        })
                                        .child(self.port.clone()),
                                ),
                            )
                            .child(
                                // TLS 체크박스
                                div()
                                    .id("tls")
                                    .flex()
                                    .items_center()
                                    .gap_x(px(8.))
                                    .pb(px(8.))
                                    .text_sm()
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                        this.set_tls(!this.tls, cx);
                                    }))
                                    .child(
                                        div()
                                            .size(px(16.))
                                            .rounded(px(3.))
                                            .border_1()
                                            .border_color(theme.border)
                                            .when(self.tls, |d| {
                                                d.bg(theme.accent)
                                                    .child(
                                                        div()
                                                            .text_color(theme.text_inverse)
                                                            .text_size(px(11.))
                                                            .child("✓"),
                                                    )
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                            })
                                            .overflow_hidden(),
                                    )
                                    .child("TLS"),
                            ),
                    )
                    .children(port_error.map(|text| error_text(text, theme)))
                    // Nickname
                    .child(
                        div().child(field_label("닉네임", theme)).child(
                            div()
                                .mt(px(4.))
                                .rounded(px(6.))
                                .px(px(12.))
                                .py(px(8.))
                                .text_sm()
                                .bg(theme.bg_input)
                                .border_1()
                                .border_color(if self.nickname.read(cx).is_focused(window) {
                                    theme.focus_ring
                                } else {
                                    theme.border
                                })
                                .child(self.nickname.clone()),
                        ),
                    )
                    .children(nick_error.map(|text| error_text(text, theme)))
                    // Actions
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_x(px(8.))
                            .pt(px(16.))
                            .child(
                                // 취소
                                div()
                                    .id("cancel")
                                    .cursor_pointer()
                                    .rounded(px(6.))
                                    .px(px(16.))
                                    .py(px(8.))
                                    .text_sm()
                                    .hover(|h| h.bg(theme.menu_hover))
                                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                        this.cancel(window, cx)
                                    }))
                                    .child("취소"),
                            )
                            .child(
                                // 연결
                                div()
                                    .id("submit")
                                    .cursor_pointer()
                                    .rounded(px(6.))
                                    .px(px(16.))
                                    .py(px(8.))
                                    .text_sm()
                                    .bg(theme.accent)
                                    .text_color(theme.text_inverse)
                                    .hover(|h| h.bg(theme.accent_hover))
                                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                        this.submit(window, cx)
                                    }))
                                    .child("연결"),
                            ),
                    ),
            );

        modal_overlay(
            panel,
            theme,
            cx.listener(|this, _: &Dismiss, window, cx| this.cancel(window, cx)),
        )
    }
}
