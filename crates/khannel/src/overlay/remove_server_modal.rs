//! RemoveServerModal: 서버 삭제 확인.
//!
//! 삭제는 되돌릴 수 없으므로(저장된 서버 설정·채널 목록·잠금까지 지워진다) 서버 컨텍스트 메뉴의
//! "Delete Server"는 이 확인을 거친다. Enter = 삭제, Esc = 취소.

use gpui::{
    ClickEvent, Context, Entity, EventEmitter, FocusHandle, FontWeight, IntoElement, KeyDownEvent,
    ParentElement, Render, StatefulInteractiveElement, Styled, Window, div, prelude::*, px,
};
use khannel_core::client::types::ServerId;

use crate::action::Dismiss;
use crate::model::IrcModel;
use crate::overlay::modal::{DismissEvent, error_text, modal_overlay, modal_panel};
use crate::theme::Theme;

pub struct RemoveServerModal {
    model: Entity<IrcModel>,
    /// 삭제 대상 서버. None이면 모달이 빈 상태.
    server_id: Option<ServerId>,
    server_name: String,
    /// 삭제 실패 사유 (모달을 닫지 않고 표시)
    error: Option<String>,
    /// 입력창이 없는 모달이라 패널 자체에 포커스를 둔다 — Esc(Dismiss)/Enter가 모달에 닿게.
    focus: FocusHandle,
}

impl EventEmitter<DismissEvent> for RemoveServerModal {}

impl RemoveServerModal {
    pub fn new(model: Entity<IrcModel>, cx: &mut Context<Self>) -> Self {
        Self {
            model,
            server_id: None,
            server_name: String::new(),
            error: None,
            focus: cx.focus_handle(),
        }
    }

    /// 모달을 열 때 대상 서버를 지정한다.
    pub fn open_for(&mut self, server_id: ServerId, cx: &mut Context<Self>) {
        self.server_name = self
            .model
            .read(cx)
            .servers()
            .get(&server_id)
            .map(|server| server.name.clone())
            .unwrap_or_default();
        self.server_id = Some(server_id);
        self.error = None;
        cx.notify();
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
    }

    fn confirm(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(server_id) = self.server_id else {
            return;
        };
        match self
            .model
            .update(cx, |m, cx| m.remove_server(server_id, cx))
        {
            Ok(()) => {
                self.server_id = None;
                cx.emit(DismissEvent);
            }
            Err(e) => {
                self.error = Some(format!("서버 삭제 실패: {e}"));
                cx.notify();
            }
        }
    }

    fn cancel(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.server_id = None;
        cx.emit(DismissEvent);
    }
}

impl Render for RemoveServerModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::current(cx);

        let panel = modal_panel(px(360.), px(20.), theme)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "enter" {
                    this.confirm(window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .mb(px(12.))
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("서버 삭제"),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_y(px(12.))
                    .child(
                        div()
                            .text_sm()
                            .child(format!("'{}' 서버를 삭제할까요?", self.server_name)),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.text_subtle)
                            .child("저장된 서버 설정과 채널 목록, 잠금 설정이 함께 삭제됩니다."),
                    )
                    .children(self.error.as_deref().map(|text| error_text(text, theme)))
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
                                    .id("confirm")
                                    .cursor_pointer()
                                    .rounded(px(6.))
                                    .px(px(12.))
                                    .py(px(6.))
                                    .text_sm()
                                    .bg(theme.danger_text)
                                    .text_color(theme.text_inverse)
                                    .hover(|h| h.opacity(0.85))
                                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                        this.confirm(window, cx)
                                    }))
                                    .child("삭제"),
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
