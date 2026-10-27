//! ChannelJoinModal: 채널 입장 폼 (# 채널명 검증).
//!
//! Svelte `ChannelJoinModal.svelte`의 포팅.

use std::sync::Arc;

use gpui::{
    div, prelude::*, px, App, ClickEvent, Context, Entity, EventEmitter, Focusable, IntoElement,
    ParentElement, Render, StatefulInteractiveElement, Styled, Window,
};

use crate::action::Dismiss;
use crate::api::UiApi;
use crate::model::IrcModel;
use crate::overlay::modal::{error_text, modal_overlay, modal_panel, DismissEvent};
use crate::theme::Theme;
use crate::ui::input::{TextInput, TextInputEvent};

pub struct ChannelJoinModal {
    #[allow(dead_code)]
    api: Arc<UiApi>,
    model: Entity<IrcModel>,
    name: Entity<TextInput>,
    error: Option<&'static str>,
}

impl EventEmitter<DismissEvent> for ChannelJoinModal {}

impl ChannelJoinModal {
    pub fn new(
        api: Arc<UiApi>,
        model: Entity<IrcModel>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| TextInput::new(cx).with_placeholder("#channel"));

        cx.subscribe_in(
            &name,
            window,
            |this, _input, event: &TextInputEvent, window, cx| {
                if let TextInputEvent::Submit(_) = event {
                    this.submit(window, cx);
                }
            },
        )
        .detach();

        cx.observe(&model, |_, _, cx| cx.notify()).detach();

        Self {
            api,
            model,
            name,
            error: None,
        }
    }

    pub fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.name.update(cx, |input, cx| input.clear(window, cx));
        self.error = None;
        cx.notify();
    }

    pub fn focus_first(&self, window: &mut Window, cx: &mut App) {
        let focus = self.name.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
    }

    fn validate(&self, name: &str) -> Option<&'static str> {
        // Svelte validate()와 동일 규칙
        if name.is_empty() {
            Some("채널 이름을 입력하세요")
        } else if !name.starts_with('#') {
            Some("#으로 시작해야 합니다")
        } else {
            None
        }
    }

    fn submit(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let name = self.name.read(cx).text().trim().to_string();

        if let Some(error) = self.validate(&name) {
            self.error = Some(error);
            cx.notify();
            return;
        }

        let server_id = self.model.read(cx).current_server_id().copied();
        let Some(server_id) = server_id else { return };

        self.model
            .update(cx, |m, cx| m.join_channel(server_id, &name, cx));
        cx.emit(DismissEvent);
    }

    fn cancel(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }
}

impl Render for ChannelJoinModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::current(cx);

        let panel = modal_panel(px(384.), px(20.), theme) // max-w-sm p-5
            .child(
                // Svelte: header mb-4 text-lg font-semibold
                div()
                    .mb(px(16.))
                    .text_lg()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("채널 추가"),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_y(px(12.)) // space-y-3
                    .child(
                        div()
                            .rounded(px(6.))
                            .px(px(12.))
                            .py(px(8.))
                            .text_sm()
                            .bg(theme.bg_input)
                            .border_1()
                            .border_color(if self.name.read(cx).is_focused(window) {
                                theme.focus_ring
                            } else {
                                theme.border
                            })
                            .child(self.name.clone()),
                    )
                    .children(self.error.map(|text| error_text(text, theme)))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_x(px(8.))
                            .pt(px(12.))
                            .child(
                                div()
                                    .id("cancel")
                                    .cursor_pointer()
                                    .rounded(px(6.))
                                    .px(px(12.))
                                    .py(px(6.))
                                    .text_sm()
                                    .hover(|h| h.bg(theme.menu_hover))
                                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                        this.cancel(window, cx)
                                    }))
                                    .child("취소"),
                            )
                            .child(
                                div()
                                    .id("submit")
                                    .cursor_pointer()
                                    .rounded(px(6.))
                                    .px(px(12.))
                                    .py(px(6.))
                                    .text_sm()
                                    .bg(theme.accent)
                                    .text_color(theme.text_inverse)
                                    .hover(|h| h.bg(theme.accent_hover))
                                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                        this.submit(window, cx)
                                    }))
                                    .child("추가"),
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
