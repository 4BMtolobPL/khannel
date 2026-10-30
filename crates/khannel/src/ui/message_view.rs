//! MessageView: 메시지 목록 (자동 스크롤 + UnreadDivider).
//!
//! Svelte `MessageView.svelte`/`UnreadDivider.svelte`의 포팅:
//! - 대량 메시지를 위한 GPUI `list` 가상화 (docs/MIGRATION.md 리스크 완화: "GPUI list 기반 가상화")
//! - `FollowMode::Tail` → Svelte의 `isBottom` 자동 스크롤과 동일
//!   (아래에서 위로 스크롤했다가 다시 맨 아래로 오면 following 재개)
//! - 채널 전환 시 `lastReadMessageId` 위치로 스크롤 복원 (scrollIntoView block:"start" 동일)
//! - 맨 아래 스크롤 중에는 `lastReadMessageId`를 마지막 메시지로 갱신 (onScroll 로직)
//! - Unread divider는 Svelte와 달리 lastRead를 실시간으로 따르지 않고, 채널 진입 시점의
//!   첫 미확인 메시지 위치에 고정해 그 채널을 보는 동안 유지한다. (실시간 방식은 미확인
//!   메시지가 화면 안에 다 들어오면 진입 즉시 lastRead가 갱신되어 divider가 사라졌다.)

use gpui::{
    Context, Entity, FontWeight, IntoElement, ListAlignment, ListState, ParentElement, Render,
    Styled, Window, div, list, prelude::*, px,
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
    /// 이번 채널 방문 동안 unread divider를 그릴 위치(이 index의 메시지 바로 위).
    /// 채널 진입 시 `first_unread_index`로 계산하고, 방문 중에는 위로 스크롤해 둔 상태에서
    /// 새 메시지가 오면 `unread_from`으로 생성한다(이미 있으면 유지).
    divider_before: Option<usize>,
}

/// 첫 미확인 메시지 index — lastRead 다음 메시지(lastRead가 없으면 처음)부터를 미확인으로 본다.
///
/// 미확인 구간에 (내 것이 아닌) 유저 메시지가 있을 때만 Some (unread 배지와 같은 기준 —
/// join/part 같은 시스템 메시지만 쌓였다면 divider를 만들지 않는다). lastRead ID가 목록에 없으면 None.
fn first_unread_index(
    messages: &[crate::model::ChatMessage],
    last_read: Option<&str>,
    own_nickname: Option<&str>,
) -> Option<usize> {
    let start = match last_read {
        Some(id) => messages.iter().position(|message| message.id == id)? + 1,
        None => 0,
    };
    unread_from(messages, start, own_nickname)
}

/// `start`부터를 미확인 구간으로 볼 때의 divider 위치 — 구간에 다른 사람의 유저 메시지가
/// 있을 때만 Some(start). 내가 보낸 메시지(에코)는 미확인으로 치지 않는다.
fn unread_from(
    messages: &[crate::model::ChatMessage],
    start: usize,
    own_nickname: Option<&str>,
) -> Option<usize> {
    messages
        .get(start..)?
        .iter()
        .any(|message| {
            let is_own = matches!(
                (own_nickname, message.nickname.as_deref()),
                (Some(own), Some(nick)) if crate::model::nick_eq(own, nick)
            );
            message.kind.is_user_message() && !is_own
        })
        .then_some(start)
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

        // 모델 변경(새 메시지 등) → Tail following 중이면 lastRead를 마지막 메시지로 갱신.
        // (Svelte: 자동 스크롤이 onScroll → lastRead 갱신을 유발하는 것과 동일 효과)
        // 렌더 중이 아니라 observer에서 갱신한다 — 렌더 시 모델을 수정하지 않기 위함.
        cx.observe(&model, |this, _, cx| {
            this.mark_read_if_following(cx);
            cx.notify();
        })
        .detach();

        Self {
            model,
            list_state,
            rendered: None,
            divider_before: None,
        }
    }

    /// Tail following 중(맨 아래)이면 현재 채널의 lastRead를 마지막 메시지로 갱신한다.
    ///
    /// 마지막으로 렌더한 채널과 현재 채널이 같을 때만 동작한다. 채널 전환 직후에는
    /// ListState가 아직 이전 채널 기준으로 following 상태라, 그대로 갱신하면 새 채널의
    /// lastRead를 덮어써 안 읽은 위치 복원(`sync_list`)이 항상 맨 아래로 가 버린다.
    fn mark_read_if_following(&self, cx: &mut Context<Self>) {
        if !self.list_state.is_following_tail() {
            return;
        }
        let model = self.model.read(cx);
        let Some(channel_id) = model.current_channel_id().map(str::to_string) else {
            return;
        };
        let rendered_same_channel = self
            .rendered
            .as_ref()
            .is_some_and(|(rendered, _)| *rendered == channel_id);
        if !rendered_same_channel {
            return;
        }
        let Some(last_id) = model
            .current_messages()
            .and_then(|messages| messages.last().map(|message| message.id.clone()))
        else {
            return;
        };

        self.model.update(cx, |m, cx| {
            m.update_channel_last_read_message_id(&channel_id, Some(last_id), cx)
        });
    }

    /// 렌더 직전에 ListState를 모델과 동기화한다 (ListState만 갱신, 모델은 수정하지 않음).
    /// 표시 버퍼는 채널 또는 (채널 미선택 시) 서버 버퍼다 — `IrcModel::visible_messages`.
    /// - 버퍼 전환: reset + 마지막 읽은 위치(또는 맨 아래)로 스크롤 복원
    /// - 메시지 추가: splice로 항목 수 반영 (Tail following은 유지)
    fn sync_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let channel_id = self.model.read(cx).visible_buffer_key();
        let count = self
            .model
            .read(cx)
            .visible_messages()
            .map(|messages| messages.len())
            .unwrap_or(0);

        let same_channel = self
            .rendered
            .as_ref()
            .is_some_and(|(prev, _)| Some(prev) == channel_id.as_ref());

        if same_channel {
            let prev_count = self.rendered.as_ref().map(|(_, count)| *count);
            if let Some(prev_count) = prev_count
                && count > prev_count
            {
                // 실시간 divider: 위로 스크롤해 둔 상태(following 아님)에서 새 메시지가 오면
                // 새로 들어온 첫 메시지 위에 divider를 만든다. 맨 아래에서 따라가는 중이면
                // 바로 읽은 것으로 보아 만들지 않는다. (following 여부는 직전 레이아웃 기준)
                if self.divider_before.is_none() && !self.list_state.is_following_tail() {
                    let model = self.model.read(cx);
                    let own_nickname = model.current_server().map(|s| s.nickname.as_str());
                    self.divider_before = model
                        .visible_messages()
                        .and_then(|messages| unread_from(messages, prev_count, own_nickname));
                }
                // splice의 두 번째 인자는 "새로 끼워 넣을 항목 수"다.
                // (이전: 전체 개수 count를 넘겨 메시지마다 list 항목이 count개씩 불어났다)
                self.list_state
                    .splice(prev_count..prev_count, count - prev_count);
            }
        } else {
            // 채널 전환(또는 첫 렌더): divider 위치 고정 + 스크롤 복원
            let model = self.model.read(cx);
            let own_nickname = model.current_server().map(|s| s.nickname.as_str());
            self.divider_before = model.current_channel().and_then(|channel| {
                first_unread_index(
                    model.current_messages()?,
                    channel.last_read_message_id.as_deref(),
                    own_nickname,
                )
            });

            self.list_state.reset(count);
            if let Some(index) = self.divider_before {
                // Svelte restoreScroll: 마지막 읽은 메시지를 맨 위로 (scrollIntoView block:"start").
                // 미확인 메시지가 화면 안에 다 들어오면 레이아웃에서 Tail following이 재개된다.
                self.list_state.pause_following_tail();
                self.list_state.scroll_to(gpui::ListOffset {
                    item_ix: index.saturating_sub(1),
                    offset_in_item: px(0.),
                });
            } else {
                self.list_state.set_follow_mode(gpui::FollowMode::Tail);
            }

            // 이번 프레임 레이아웃이 끝나야 following 여부가 확정된다(맨 아래로 복원되면
            // 레이아웃에서 following이 재개됨) → 다음 프레임에 lastRead 갱신.
            // (divider는 위에서 고정했으므로 갱신돼도 이번 방문 동안은 사라지지 않는다.)
            cx.on_next_frame(window, |this, _, cx| this.mark_read_if_following(cx));
        }
        self.rendered = Some((channel_id.unwrap_or_default(), count));
    }

    /// Unread divider: 메시지 영역 너비 전체의 실선 + 가운데 문구.
    fn render_divider(theme: Theme) -> gpui::Div {
        let line = || div().flex_1().h(px(1.)).bg(theme.accent_text);
        div()
            .w_full()
            .flex()
            .items_center()
            .gap_x(px(8.))
            .px(px(4.))
            .my(px(6.))
            .child(line())
            .child(
                div()
                    .flex_shrink_0()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.accent_text)
                    .child("Last read"),
            )
            .child(line())
    }

    fn render_item(
        message: &crate::model::ChatMessage,
        show_divider: bool,
        own_nickname: &Option<String>,
        theme: Theme,
    ) -> gpui::AnyElement {
        let row = match message.kind {
            MessageType::User => {
                // Svelte: 내 닉네임이면 `< {nick}>`, 아니면 `<@{nick}>`
                let is_own = matches!(
                    (own_nickname, &message.nickname),
                    (Some(own), Some(nick)) if crate::model::nick_eq(own, nick)
                );
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
            MessageType::Action => {
                // `* nick 내용` — 닉과 본문을 한 문장처럼 이탤릭으로
                let nick = message.nickname.clone().unwrap_or_default();
                div()
                    .mb_1()
                    .flex()
                    .min_w_0()
                    .italic()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .flex_shrink_0()
                            .whitespace_nowrap()
                            .text_color(theme.accent_text)
                            .child(format!("* {nick}")),
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
            // divider는 첫 미확인 메시지 바로 위에 그린다
            .when(show_divider, |d| d.child(Self::render_divider(theme)))
            .child(row)
            .into_any()
    }
}

impl Render for MessageView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_list(window, cx);

        let theme = Theme::current(cx);
        let model = self.model.read(cx);
        let own_nickname = model.current_server().map(|server| server.nickname.clone());
        let model_for_items = self.model.clone();
        let divider_before = self.divider_before;

        // Svelte p-3: 메시지 영역 패딩
        div()
            .flex() // div 기본 display는 Block — flex_1/flex_col은 display:flex여야 동작
            .flex_1()
            .min_h_0()
            .flex_col()
            .overflow_hidden()
            .p(px(12.))
            .child(
                // 메시지 목록을 clone하지 않고, 가상화된 list가 실제로 그리는 항목만
                // 모델에서 직접 읽는다 (이전: 렌더마다 Vec 전체 clone → 메시지당 O(n)).
                list(self.list_state.clone(), move |index, _window, cx| {
                    let Some(messages) = model_for_items.read(cx).visible_messages() else {
                        return div().into_any();
                    };
                    let Some(message) = messages.get(index) else {
                        return div().into_any();
                    };
                    Self::render_item(message, divider_before == Some(index), &own_nickname, theme)
                })
                .flex_1(), // 부모가 flex (column)이므로 남은 높이를 채운다
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ChatMessage;

    fn message(id: &str, kind: MessageType) -> ChatMessage {
        ChatMessage {
            kind,
            id: id.to_string(),
            nickname: None,
            content: String::new(),
            timestamp: 0,
        }
    }

    fn messages() -> Vec<ChatMessage> {
        vec![
            message("a", MessageType::User),
            message("b", MessageType::User),
            message("c", MessageType::System),
            message("d", MessageType::User),
        ]
    }

    #[test]
    fn divider_after_last_read_when_user_messages_follow() {
        // 화면 안에 다 들어오는 소수의 미확인 메시지여도 divider 위치가 나온다
        assert_eq!(first_unread_index(&messages(), Some("a"), None), Some(1));
        assert_eq!(first_unread_index(&messages(), Some("c"), None), Some(3));
    }

    #[test]
    fn no_divider_when_everything_read_or_only_system_messages() {
        assert_eq!(first_unread_index(&messages(), Some("d"), None), None);
        let system_tail = vec![
            message("a", MessageType::User),
            message("b", MessageType::System),
        ];
        assert_eq!(first_unread_index(&system_tail, Some("a"), None), None);
    }

    #[test]
    fn never_read_channel_starts_at_top() {
        assert_eq!(first_unread_index(&messages(), None, None), Some(0));
        assert_eq!(first_unread_index(&[], None, None), None);
    }

    #[test]
    fn live_divider_only_for_new_user_messages() {
        // 새로 들어온 구간(index 2..)에 유저 메시지가 있으면 그 시작 위치
        assert_eq!(unread_from(&messages(), 2, None), Some(2));
        // 새로 들어온 게 시스템 메시지뿐이면 없음
        let system_tail = vec![
            message("a", MessageType::User),
            message("b", MessageType::System),
        ];
        assert_eq!(unread_from(&system_tail, 1, None), None);
        assert_eq!(unread_from(&messages(), 9, None), None);
    }

    #[test]
    fn own_messages_are_not_unread() {
        let mut own = message("me", MessageType::User);
        own.nickname = Some("kim".to_string());
        let mut other = message("you", MessageType::User);
        other.nickname = Some("lee".to_string());

        // 위로 스크롤한 채 내가 보낸 메시지만 들어오면 divider 없음
        let list = vec![message("a", MessageType::User), own.clone()];
        assert_eq!(unread_from(&list, 1, Some("kim")), None);
        // 다른 사람 메시지가 섞이면 새 구간 시작 위치
        let list = vec![message("a", MessageType::User), own, other];
        assert_eq!(unread_from(&list, 1, Some("kim")), Some(1));
    }

    #[test]
    fn action_messages_count_as_unread() {
        let list = vec![
            message("a", MessageType::User),
            message("b", MessageType::Action),
        ];
        assert_eq!(unread_from(&list, 1, None), Some(1));
    }

    #[test]
    fn unknown_last_read_has_no_divider() {
        assert_eq!(first_unread_index(&messages(), Some("missing"), None), None);
    }
}
