//! ChangeNickModal: 닉네임 변경 폼 (에러 표시 + 성공 시 자동 닫힘).
//!
//! Svelte `ChangeNicknameModal.svelte`의 포팅:
//! - 에러: `ircStore.nickErrors.get(serverId)` → IrcModel.nick_error
//! - 성공: `nickSuccess` 감지 시 모달 닫기 + 캐시 삭제
//! - 닫힐 때: nickErrors 삭제 + 입력 초기화

use gpui::{
    App, ClickEvent, Context, Entity, EventEmitter, Focusable, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, Window, div, prelude::*, px,
};
use khannel_core::client::types::ServerId;

use crate::action::Dismiss;
use crate::model::IrcModel;
use crate::overlay::modal::{DismissEvent, error_text, modal_overlay, modal_panel};
use crate::theme::Theme;
use crate::ui::input::{TextInput, TextInputEvent};

pub struct ChangeNickModal {
    model: Entity<IrcModel>,
    /// 닉변 대상 서버 (Svelte: serverContextMenu.serverId). None이면 모달이 빈 상태.
    server_id: Option<ServerId>,
    nick: Entity<TextInput>,
}

impl EventEmitter<DismissEvent> for ChangeNickModal {}

impl ChangeNickModal {
    pub fn new(model: Entity<IrcModel>, window: &Window, cx: &mut Context<Self>) -> Self {
        let nick = cx.new(|cx| TextInput::new(cx).with_placeholder("새 닉네임"));

        cx.subscribe_in(
            &nick,
            window,
            |this, _input, event: &TextInputEvent, window, cx| {
                if let TextInputEvent::Submit(_) = event {
                    this.submit(window, cx);
                }
            },
        )
        .detach();

        // 성공 감지 → 모달 닫기 (Svelte: $effect에서 nickSuccess 확인)
        // 모달이 닫혀 있으면 server_id가 None이라 반응하지 않는다.
        cx.observe(&model, |this, model, cx| {
            let server_id = this.server_id;
            if let Some(server_id) = server_id
                && model
                    .update(cx, |m, _cx| m.take_nick_success(&server_id))
                    .is_some()
            {
                this.clear_error(cx);
                this.server_id = None;
                cx.emit(DismissEvent);
            }
        })
        .detach();

        Self {
            model,
            server_id: None,
            nick,
        }
    }

    /// 모달을 열 때 대상 서버를 지정하고 입력을 초기화한다.
    pub fn open_for(
        &mut self,
        server_id: Option<ServerId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.server_id = server_id;
        self.nick.update(cx, |input, cx| input.clear(window, cx));
        // 이전 실패 에러 + 요청/성공 기록 제거 (Svelte: 모달 열림/닫힘 effect에서 nickErrors 정리).
        // 성공 기록이 남아 있으면 열린 직후 첫 모델 변경에 모달이 닫혀 버린다.
        if let Some(server_id) = server_id {
            self.model.update(cx, |m, cx| {
                m.clear_nick_error(&server_id, cx);
                m.reset_nick_change(&server_id);
            });
        }
        cx.notify();
    }

    pub fn focus_first(&self, window: &mut Window, cx: &mut App) {
        let focus = self.nick.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
    }

    fn clear_error(&self, cx: &mut App) {
        if let Some(server_id) = self.server_id {
            self.model
                .update(cx, |m, cx| m.clear_nick_error(&server_id, cx));
        }
    }

    fn submit(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(server_id) = self.server_id else {
            return;
        };
        let new_nick = self.nick.read(cx).text().trim().to_string();
        if new_nick.is_empty() {
            return;
        }

        // Svelte changeNickSubmit: 제출 전 기존 에러 제거
        self.model
            .update(cx, |m, cx| m.clear_nick_error(&server_id, cx));
        self.model
            .update(cx, |m, cx| m.change_nickname(server_id, &new_nick, cx));
    }

    fn cancel(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.clear_error(cx);
        // 닫힌 뒤에는 닉 변경 이벤트에 반응하지 않도록 대상 서버를 해제한다
        self.server_id = None;
        cx.emit(DismissEvent);
    }
}

impl Render for ChangeNickModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::current(cx);
        let error = self.server_id.and_then(|server_id| {
            self.model
                .read(cx)
                .nick_error(&server_id)
                .map(str::to_string)
        });

        let panel = modal_panel(px(320.), px(16.), theme) // w-80 p-4
            .child(
                // Svelte: header mb-4 text-lg font-semibold
                div()
                    .mb(px(16.))
                    .text_lg()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("닉네임 변경"),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_y(px(12.))
                    .child(
                        div()
                            .rounded(px(6.))
                            .px(px(12.))
                            .py(px(8.))
                            .text_sm()
                            .bg(theme.bg_input)
                            .border_1()
                            .border_color(if self.nick.read(cx).is_focused(window) {
                                theme.focus_ring
                            } else {
                                theme.border
                            })
                            .child(self.nick.clone()),
                    )
                    .children(error.map(|text| error_text(&text, theme)))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_x(px(8.))
                            .pt(px(16.))
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
                                    .child("변경"),
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
