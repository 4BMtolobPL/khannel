//! InputBar: 잠금 토글 + 메시지 입력 + Send 버튼.
//!
//! Svelte `+page.svelte`의 `<section>` 입력 영역 포팅. Phase 2의 키 네이밍 기반
//! 간이 composer를 IME 지원 TextInput 엔티티로 교체했다 (MIGRATION.md Phase 3).

use std::sync::Arc;

use gpui::{
    div, prelude::*, px, App, ClickEvent, Context, Entity, Focusable, IntoElement, ParentElement,
    Render, StatefulInteractiveElement, Styled, Window,
};

use crate::api::UiApi;
use crate::model::IrcModel;
use crate::theme::Theme;
use crate::ui::input::{TextInput, TextInputEvent};

pub struct InputBar {
    #[allow(dead_code)]
    api: Arc<UiApi>,
    model: Entity<IrcModel>,
    input: Entity<TextInput>,
}

impl InputBar {
    pub fn new(
        api: Arc<UiApi>,
        model: Entity<IrcModel>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| TextInput::new(cx).with_placeholder("Type a message"));

        // Enter 제출 → 메시지 전송 + 입력 클리어 (Svelte sendMessage)
        cx.subscribe_in(
            &input,
            window,
            |this, _input, event: &TextInputEvent, window, cx| {
                if let TextInputEvent::Submit(text) = event {
                    this.send_text(text.to_string(), window, cx);
                }
            },
        )
        .detach();

        // 잠금 상태 → 입력 비활성화/플레이스홀더 갱신 (Svelte disabled/placeholder)
        cx.observe(&model, |this, model, cx| {
            let locked = model.read(cx).is_locked();
            this.input.update(cx, |input, cx| {
                input.set_enabled(!locked, cx);
                input.set_placeholder(
                    if locked {
                        "Channel is locked"
                    } else {
                        "Type a message"
                    },
                    cx,
                );
            });
            cx.notify();
        })
        .detach();

        Self { api, model, input }
    }

    /// composer 입력창 포커스 핸들 (ChatApp의 자동 포커스용)
    pub fn input_focus_handle(&self, cx: &App) -> gpui::FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }

    /// 입력창이 포커스 중인지 (ChatApp 자동 포커스 판단용)
    pub fn input_focused(&self, window: &Window, cx: &App) -> bool {
        self.input.read(cx).is_focused(window)
    }

    /// Send 버튼 클릭 경로: 입력창의 현재 텍스트로 전송한다.
    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.input.read(cx).text().to_string();
        self.send_text(text, window, cx);
    }

    fn send_text(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        if text.trim().is_empty() {
            return;
        }
        self.model.update(cx, |m, cx| m.send_message(&text, cx));
        self.input.update(cx, |input, cx| input.clear(window, cx));
    }

    fn toggle_lock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.model.update(cx, |m, cx| m.toggle_lock(cx));
        // Svelte toggleLock이 msgInput을 클리어하는 것과 동일.
        self.input.update(cx, |input, cx| input.clear(window, cx));
    }

    fn render_button(
        &self,
        id: &'static str,
        label: &'static str,
        theme: Theme,
    ) -> gpui::Stateful<gpui::Div> {
        // Svelte: text-sm px-2 py-1 rounded hover:bg-neutral-200/700
        // 이모지 아이콘이 정중앙에 오도록 flex 가운데 정렬
        div()
            .id(id)
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .px(px(8.))
            .py(px(4.))
            .text_sm()
            .hover(|h| h.bg(theme.hover))
            .child(label)
    }
}

impl Render for InputBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::current(cx);
        let is_locked = self.model.read(cx).is_locked();

        // Svelte: border-t p-3 + form flex gap-2
        div()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.panel)
            .p(px(12.))
            .child(
                div()
                    .flex()
                    .gap_x(px(8.))
                    .child(
                        self.render_button("lock", if is_locked { "🔒" } else { "🔓" }, theme)
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.toggle_lock(window, cx)
                            })),
                    )
                    .child(
                        // 입력 상자: flex-1 rounded px-3 py-2 text-sm border + focus ring
                        div()
                            .flex_1()
                            .min_w_0()
                            .rounded(px(6.))
                            .px(px(12.))
                            .py(px(8.))
                            .text_sm()
                            .bg(theme.bg_input)
                            .border_1()
                            .border_color(if self.input.read(cx).is_focused(window) {
                                theme.focus_ring
                            } else {
                                theme.border
                            })
                            .child(self.input.clone()),
                    )
                    .child(
                        // Send: rounded px-4 py-2 text-sm bg-sky-600 text-white hover:bg-sky-500
                        div()
                            .id("send")
                            .cursor_pointer()
                            .rounded(px(4.))
                            .px(px(16.))
                            .py(px(8.))
                            .text_sm()
                            .bg(theme.accent)
                            .text_color(theme.text_inverse)
                            .when(!is_locked, |d| d.hover(|h| h.bg(theme.accent_hover)))
                            .when(is_locked, |d| d.opacity(0.4))
                            .on_click(
                                cx.listener(|this, _: &ClickEvent, window, cx| {
                                    this.send(window, cx)
                                }),
                            )
                            .child("Send"),
                    ),
            )
    }
}
