//! Sidebar: 서버/채널 리스트 + 추가 버튼 (+ 우클릭 컨텍스트 메뉴 요청).
//!
//! Svelte `+page.svelte`의 `<aside>`를 1:1 포팅. 선택 상태는 IrcModel이 소유하고,
//! 모달/컨텍스트 메뉴 열기는 이벤트로 상위(ChatApp)에 위임한다 — Svelte에서
//! `+page.svelte`가 직접 들고 있던 `showServerModal`/`showChannelModal`/컨텍스트 메뉴
//! state에 해당한다.

use gpui::{
    div, prelude::*, px, App, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    IntoElement, MouseButton, MouseDownEvent, ParentElement, Pixels, Point, Render, Styled, Window,
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

// ── 행 기하 (zeron sidebar 수치) ──
/// 섹션 헤더 높이 (zeron `SIDEBAR_DISCLOSURE_HEADER_HEIGHT`)
const SECTION_HEADER_HEIGHT: Pixels = px(28.);
/// 서버 행 높이 (zeron compact row 29px)
const ROW_HEIGHT: Pixels = px(29.);
/// 채널 행 높이 (서버 아래 한 단계 들여쓴 하위 행)
const CHILD_ROW_HEIGHT: Pixels = px(26.);
/// 행 모서리 / 가로 패딩 (zeron session row: r8, `SPACE_SM`)
const ROW_RADIUS: Pixels = px(8.);
const ROW_PAD_X: Pixels = px(8.);
/// 목록 항목 간격 / 위쪽 여백 (zeron `SIDEBAR_LIST_GAP`, `SIDEBAR_LIST_PAD_TOP`)
const LIST_GAP: Pixels = px(2.);
const LIST_PAD_TOP: Pixels = px(4.);
/// 목록 좌우 여백 (행 hover/선택 배경이 사이드바 가장자리에 붙지 않게)
const LIST_PAD_X: Pixels = px(6.);
/// 채널 행 들여쓰기 — 서버 행의 상태 점 다음, 이름 시작 위치에 맞춘다
const CHILD_INDENT: Pixels = px(14.);
/// 행 텍스트 크기 (zeron ui 13px)
const ROW_TEXT: Pixels = px(13.);

/// unread 배지 (16px 높이 알약). 알림 성격이라 zeron의 흐린 count 배지와 달리 강조색을 유지한다.
fn badge(unread: u32, theme: Theme) -> gpui::Div {
    div()
        .flex_none()
        .h(px(16.))
        .min_w(px(16.))
        .px(px(5.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(theme.badge_bg)
        .text_color(theme.badge_text)
        .text_size(px(10.))
        .font_weight(FontWeight::MEDIUM)
        .child(if unread > 99 {
            "99+".to_string()
        } else {
            unread.to_string()
        })
}

/// 사이드바 행 공통 모양: 선택이면 wash 배경 + 본래 글자색, 아니면 80% 글자 + hover wash.
/// (zeron session row — 선택 링 없이 wash만으로 표시)
fn row(
    id: impl Into<gpui::ElementId>,
    height: Pixels,
    selected: bool,
    theme: Theme,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .w_full()
        .h(height)
        .flex_none()
        .flex()
        .items_center()
        .gap_x(px(8.))
        .px(ROW_PAD_X)
        .rounded(ROW_RADIUS)
        .cursor_pointer()
        .text_size(ROW_TEXT)
        .map(|d| {
            if selected {
                d.bg(theme.row_selected()).text_color(theme.text_main)
            } else {
                d.text_color(Theme::fade(theme.text_main, 0.8))
                    .hover(|h| h.bg(theme.row_hover()).text_color(theme.text_main))
            }
        })
}

/// 행 안의 이름 (넘치면 잘림)
fn row_label(text: String) -> gpui::Div {
    div()
        .flex_1()
        .min_w_0()
        .overflow_x_hidden()
        .whitespace_nowrap()
        .child(text)
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
        let header_glyph = Theme::fade(theme.text_main, 0.45);

        // 배경 톤/오른쪽 경계선은 ChatApp의 사이드바 열(창 전체 높이)이 그린다.
        div()
            .id("sidebar")
            .w(px(224.)) // Svelte w-56
            .flex_shrink_0()
            // ChatApp이 [위쪽 여백 + 사이드바] 세로 컨테이너로 감싸므로 남은 높이를 채운다
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .px(LIST_PAD_X)
            .pt(LIST_PAD_TOP)
            // ── 섹션 헤더: "Servers" + 서버 추가(+) (zeron sidebar_disclosure_header) ──
            .child(
                div()
                    .flex_none()
                    .h(SECTION_HEADER_HEIGHT)
                    .px(ROW_PAD_X)
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(header_glyph)
                            .child("Servers"),
                    )
                    .child(
                        div()
                            .id("add-server")
                            .size(px(20.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .cursor_pointer()
                            .text_color(header_glyph)
                            .hover(|h| h.bg(theme.row_hover()).text_color(theme.text_main))
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(SidebarEvent::OpenServerModal);
                            }))
                            .child("+"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_y(LIST_GAP)
                    .children(servers.iter().map(|item| {
                        let is_current = Some(item.id) == current_server_id;
                        // 선택 표시는 한 행만: 서버 버퍼(채널 미선택)를 볼 때만 서버 행이 선택된다
                        let is_selected = is_current && current_channel_id.is_none();
                        let unread = server_unread.get(&item.id).copied().unwrap_or(0);
                        let server_id = item.id;
                        let is_connected = item.status == UiServerStatus::Connected;
                        let is_connecting = item.status == UiServerStatus::Connecting
                            || item.status == UiServerStatus::Registering;

                        let server_row = row(
                            format!("server-row-{server_id}"),
                            ROW_HEIGHT,
                            is_selected,
                            theme,
                        )
                        .on_click({
                            let model = self.model.clone();
                            move |_, _, cx| {
                                model.update(cx, |m, cx| m.select_server(server_id, cx));
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
                        // 상태 점 (행의 아이콘 자리)
                        .child(
                            div()
                                .flex_none()
                                .size(px(6.))
                                .rounded_full()
                                .bg(status_dot_color(&theme, is_connected, is_connecting)),
                        )
                        .child(row_label(item.name.clone()))
                        .when(!is_current && unread > 0, |d| d.child(badge(unread, theme)));

                        div()
                            .id(server_id.to_string())
                            .flex()
                            .flex_col()
                            .gap_y(LIST_GAP)
                            .child(server_row)
                            // 현재 서버의 채널 리스트 (Svelte: li 내부에 렌더)
                            .when(is_current, |d| {
                                d.child(self.render_channel_list(
                                    server_id,
                                    &current_channel_id,
                                    theme,
                                    cx,
                                ))
                            })
                    })),
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
            .gap_y(LIST_GAP)
            .pl(CHILD_INDENT)
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

                row(channel_id.clone(), CHILD_ROW_HEIGHT, is_selected, theme)
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
                    .child(row_label(channel_name))
                    .when(unread > 0, |d| d.child(badge(unread, theme)))
            }));

        // + 채널 추가 (Svelte: 채널 리스트 마지막 항목) — 흐린 하위 행
        list.child(
            row("add-channel", CHILD_ROW_HEIGHT, false, theme)
                .text_color(Theme::fade(theme.text_main, 0.45))
                .on_click(cx.listener(|_, _, _, cx| {
                    cx.emit(SidebarEvent::OpenChannelModal);
                }))
                .child("+ 채널 추가"),
        )
    }
}
