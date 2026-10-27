//! MessageView: 메시지 목록 (자동 스크롤 + UnreadDivider).
//!
//! Svelte `MessageView.svelte`/`UnreadDivider.svelte`의 포팅:
//! - 대량 메시지를 위한 GPUI `list` 가상화 (MIGRATION.md 리스크 완화: "GPUI list 기반 가상화")
//! - `FollowMode::Tail` → Svelte의 `isBottom` 자동 스크롤과 동일
//!   (아래에서 위로 스크롤했다가 다시 맨 아래로 오면 following 재개)
//! - 채널 전환 시 `lastReadMessageId` 위치로 스크롤 복원 (scrollIntoView block:"start" 동일)
//! - 맨 아래 스크롤 중에는 `lastReadMessageId`를 마지막 메시지로 갱신 (onScroll 로직)

use gpui::{
    div, list, prelude::*, px, Context, Entity, FontWeight, IntoElement, ListAlignment, ListState,
    ParentElement, Render, Styled, Window,
};

use crate::model::{IrcModel, MessageType};
use crate::theme::Theme;

/// overdraw: 화면 밖 위/아래로 미리 렌더링할 높이.
const OVERDRAW: f32 = 300.;

pub struct MessageView {
    model: Entity<IrcModel>,
    list_state: ListState,
    /// 마지막으로 렌더한 (채널 ID, 메시지 수) — 채널 전환/추가 감지용
    rendered: Option<(String, usize)>,
}

impl MessageView {
    pub fn new(model: Entity<IrcModel>, cx: &mut Context<Self>) -> Self {
        let list_state = ListState::new(0, ListAlignment::Top, px(OVERDRAW));
        // Svelte의 isBottom 자동 스크롤: 새 메시지가 오면 맨 아래에서 따라간다.
        list_state.set_follow_mode(gpui::FollowMode::Tail);

        // onScroll → 맨 아래면 lastReadMessageId 갱신.
        // N.B. 스크롤 핸들러는 ListState의 RefCell이 빌려진 상태에서 호출되므로
        // 반드시 defer로 빼야 한다 (zed thread_view.rs의 패턴).
        let model_for_scroll = model.clone();
        list_state.set_scroll_handler(move |event, _window, cx| {
            if !event.is_following_tail {
                return;
            }
            // 핸들러는 여러 번 호출되므로 defer 클로저에는 복제본을 넘긴다.
            let model_for_scroll = model_for_scroll.clone();
            cx.defer(move |cx| {
                model_for_scroll.update(cx, |m, cx| {
                    let Some(channel_id) = m.current_channel_id().map(str::to_string) else {
                        return;
                    };
                    let last_id = m
                        .current_messages()
                        .and_then(|msgs| msgs.last().map(|message| message.id.clone()));
                    if let Some(last_id) = last_id {
                        m.update_channel_last_read_message_id(&channel_id, Some(last_id), cx);
                    }
                });
            });
        });

        cx.observe(&model, |_, _, cx| cx.notify()).detach();

        Self {
            model,
            list_state,
            rendered: None,
        }
    }

    /// 렌더 직전에 ListState를 모델과 동기화한다.
    /// - 채널 전환: reset + 마지막 읽은 위치(또는 맨 아래)로 스크롤 복원
    /// - 메시지 추가: splice로 항목 수 반영 (Tail following은 유지)
    /// - Tail following 중이면 lastReadMessageId를 마지막 메시지로 갱신
    ///   (Svelte: 자동 스크롤이 onScroll → lastRead 갱신을 유발하는 것과 동일 효과)
    fn sync_list(&mut self, cx: &mut Context<Self>) {
        let channel_id = self.model.read(cx).current_channel_id().map(str::to_string);
        let count = self
            .model
            .read(cx)
            .current_messages()
            .map(|messages| messages.len())
            .unwrap_or(0);

        let same_channel = self
            .rendered
            .as_ref()
            .is_some_and(|(prev, _)| Some(prev) == channel_id.as_ref());

        if same_channel {
            let prev_count = self.rendered.as_ref().map(|(_, count)| *count);
            if let Some(prev_count) = prev_count {
                if count > prev_count {
                    self.list_state.splice(prev_count..prev_count, count);
                }
            }
        } else {
            // 채널 전환(또는 첫 렌더): 스크롤 복원
            let last_read_index = self.model.read(cx).current_channel().and_then(|channel| {
                let messages = self.model.read(cx).current_messages()?;
                let last_read = channel.last_read_message_id.clone()?;
                messages.iter().position(|message| message.id == last_read)
            });

            self.list_state.reset(count);
            if let Some(index) = last_read_index {
                // Svelte restoreScroll: scrollIntoView({block: "start"})
                self.list_state.pause_following_tail();
                self.list_state.scroll_to(gpui::ListOffset {
                    item_ix: index,
                    offset_in_item: px(0.),
                });
            } else {
                self.list_state.set_follow_mode(gpui::FollowMode::Tail);
            }
        }
        self.rendered = Some((channel_id.unwrap_or_default(), count));

        // Tail following 중(맨 아래)이면 lastRead를 마지막 메시지로 — 값 변경 시에만 notify.
        if self.list_state.is_following_tail() {
            let channel_id = self.model.read(cx).current_channel_id().map(str::to_string);
            let last_id = self
                .model
                .read(cx)
                .current_messages()
                .and_then(|messages| messages.last().map(|message| message.id.clone()));
            if let (Some(channel_id), Some(last_id)) = (channel_id, last_id) {
                self.model.update(cx, |m, cx| {
                    m.update_channel_last_read_message_id(&channel_id, Some(last_id), cx)
                });
            }
        }
    }

    fn render_item(
        message: &crate::model::ChatMessage,
        is_last: bool,
        last_read_message_id: &Option<String>,
        own_nickname: &Option<String>,
        theme: Theme,
    ) -> gpui::AnyElement {
        // UnreadDivider: 마지막 읽은 메시지 직후(단, 마지막 메시지가 아닐 때)
        let show_divider = last_read_message_id.as_ref() == Some(&message.id) && !is_last;

        let row = match message.kind {
            MessageType::User => {
                // Svelte: 내 닉네임이면 `< {nick}>`, 아니면 `<@{nick}>`
                let is_own = own_nickname.as_ref() == message.nickname.as_ref();
                let nick = message.nickname.clone().unwrap_or_default();
                let label = if is_own {
                    format!("< {nick}>")
                } else {
                    format!("<@{nick}>")
                };
                div()
                    .mb_1() // mb-1
                    .flex()
                    .min_w_0()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .flex_shrink_0()
                            .whitespace_nowrap()
                            .text_color(theme.accent_text)
                            .child(label),
                    )
                    .child(
                        div()
                            .ml_1()
                            .flex_1()
                            .min_w_0()
                            .child(message.content.clone()),
                    )
            }
            MessageType::System => div()
                .mb_1()
                .flex()
                .min_w_0()
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .flex_shrink_0()
                        .text_color(theme.text_system)
                        .child("System"),
                )
                .child(
                    div()
                        .ml_1()
                        .flex_1()
                        .min_w_0()
                        .text_color(theme.text_system)
                        .child(message.content.clone()),
                ),
        };

        div()
            .w_full()
            .child(row)
            .when(show_divider, |d| {
                d.child(
                    div().mb_1().child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text_system)
                            .child("--------Last read--------"),
                    ),
                )
            })
            .into_any()
    }
}

impl Render for MessageView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_list(cx);

        let theme = Theme::current(cx);
        let model = self.model.read(cx);
        let messages = model.current_messages().cloned().unwrap_or_default();
        let last_read_message_id = model
            .current_channel()
            .and_then(|channel| channel.last_read_message_id.clone());
        let own_nickname = model.current_server().map(|server| server.nickname.clone());

        // Svelte p-3: 메시지 영역 패딩
        div().flex_1().min_h_0().overflow_hidden().p(px(12.)).child(
            list(self.list_state.clone(), move |index, _window, _cx| {
                let Some(message) = messages.get(index) else {
                    return div().into_any();
                };
                let is_last = index + 1 >= messages.len();
                Self::render_item(
                    message,
                    is_last,
                    &last_read_message_id,
                    &own_nickname,
                    theme,
                )
            })
            .flex_1(),
        )
    }
}
