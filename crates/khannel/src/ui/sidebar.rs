//! Sidebar: 서버/채널 리스트 + 추가 버튼 (+ 우클릭 컨텍스트 메뉴 요청),
//! 하단에는 현재 채널의 접이식 유저 목록.
//!
//! Svelte `+page.svelte`의 `<aside>`를 1:1 포팅. 선택 상태는 IrcModel이 소유하고,
//! 모달/컨텍스트 메뉴 열기는 이벤트로 상위(ChatApp)에 위임한다 — Svelte에서
//! `+page.svelte`가 직접 들고 있던 `showServerModal`/`showChannelModal`/컨텍스트 메뉴
//! state에 해당한다.

use std::rc::Rc;

use gpui::{
    App, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    IntoElement, MouseButton, MouseDownEvent, ParentElement, Pixels, Point, Render, ScrollStrategy,
    Styled, UniformListScrollHandle, Window, div, prelude::*, px, relative, uniform_list,
};
use khannel_core::client::types::ServerId;

use crate::assets::ICON_CIRCLE_HELP;
use crate::model::{Channel, ChannelUser, IrcModel, Server, UiServerStatus, nick_eq};
use crate::theme::{Theme, status_dot_color};

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
    UserContextMenu {
        server_id: ServerId,
        nick: String,
        position: Point<Pixels>,
    },
}

impl EventEmitter<SidebarEvent> for Sidebar {}

// ==================================== 뷰 ====================================

pub struct Sidebar {
    model: Entity<IrcModel>,
    focus: FocusHandle,
    /// 유저 목록 접힘 (모든 채널 공통, 저장하지 않음)
    users_collapsed: bool,
    users_scroll: UniformListScrollHandle,
    /// 유저 목록을 마지막으로 그린 채널 — 채널이 바뀌면 목록을 맨 위로 되돌린다
    users_channel_id: Option<String>,
    /// 권한 기호 도움말 말풍선 표시 여부
    users_help_open: bool,
    /// 도움말 버튼 위에 마우스가 있는지 — 열린 말풍선을 버튼으로 닫을 때, 바깥 클릭 닫기와
    /// 버튼 토글이 겹쳐 다시 열리지 않게 한다
    users_help_hovered: bool,
}

impl Sidebar {
    pub fn new(model: Entity<IrcModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        Self {
            model,
            focus: cx.focus_handle(),
            users_collapsed: false,
            users_scroll: UniformListScrollHandle::new(),
            users_channel_id: None,
            users_help_open: false,
            users_help_hovered: false,
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
/// 유저 목록을 펼쳤을 때 위쪽(서버/채널) 영역이 차지하는 비율 — 나머지는 유저 목록
const SERVERS_FRACTION: f32 = 0.75;
/// 유저 행의 권한 접두사 칸 폭 (닉 시작 위치를 맞춘다)
const PREFIX_WIDTH: Pixels = px(12.);
/// 도움말 버튼 / 아이콘 크기
const HELP_BUTTON_SIZE: Pixels = px(18.);
const HELP_ICON_SIZE: Pixels = px(13.);

/// 권한 기호 설명 (높은 권한부터): 기호, 이름, 설명
const PREFIX_HELP: &[(&str, &str, &str)] = &[
    ("~", "Owner", "채널 소유자"),
    ("&", "Admin", "채널 관리자"),
    (
        "@",
        "Op",
        "채널 운영자 — 강퇴·밴, 토픽·모드 변경, 권한 부여",
    ),
    ("%", "Halfop", "일부 운영 권한 (op보다 약함)"),
    (
        "+",
        "Voice",
        "발언권 — 발언 제한(+m) 채널에서도 말할 수 있음",
    ),
];

/// 권한 기호 색: op 이상(`~&@`)은 강조색, 나머지는 흐린 색
fn prefix_color(symbol: Option<char>, theme: Theme) -> gpui::Rgba {
    match symbol {
        Some('~' | '&' | '@') => theme.accent_text,
        _ => theme.text_subtle,
    }
}

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
        // 채널 버퍼일 때만 하단 유저 목록을 둔다 (서버 버퍼/DM은 None)
        let users = self.model.read(cx).current_channel_users();
        let users_expanded = users.is_some() && !self.users_collapsed;

        // 위쪽: 서버/채널 목록 (자체 스크롤). 유저 목록을 펼쳤으면 내용과 관계없이 고정 비율을
        // 차지하고, 아니면 남은 높이를 모두 쓴다 (접힌 유저 헤더는 맨 아래에 붙는다)
        let servers_area = div()
            .id("sidebar-servers")
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .px(LIST_PAD_X)
            .pt(LIST_PAD_TOP)
            .pb(LIST_PAD_TOP)
            .map(|d| {
                if users_expanded {
                    d.flex_none().h(relative(SERVERS_FRACTION))
                } else {
                    d.flex_1()
                }
            })
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
            );

        // 배경 톤/오른쪽 경계선은 ChatApp의 사이드바 열(창 전체 높이)이 그린다.
        div()
            .id("sidebar")
            .w(px(224.)) // Svelte w-56
            .flex_shrink_0()
            // ChatApp이 [위쪽 여백 + 사이드바] 세로 컨테이너로 감싸므로 남은 높이를 채운다
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(servers_area)
            .when_some(users, |d, (server_id, users)| {
                d.child(self.render_users_section(server_id, users, theme, cx))
            })
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

impl Sidebar {
    /// 하단 유저 목록: 접기/펼치기 헤더 + 가상화 목록 (수천 명 채널 대비 `uniform_list`).
    fn render_users_section(
        &mut self,
        server_id: ServerId,
        users: Vec<ChannelUser>,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        // 채널을 바꾸면 이전 채널의 스크롤 위치를 이어받지 않게 맨 위로
        let channel_id = self.model.read(cx).current_channel_id().map(str::to_string);
        if channel_id != self.users_channel_id {
            self.users_channel_id = channel_id;
            self.users_scroll.scroll_to_item(0, ScrollStrategy::Top);
            self.users_help_open = false;
        }

        let collapsed = self.users_collapsed;
        let header_glyph = Theme::fade(theme.text_main, 0.45);
        let count = users.len();

        let header = div()
            .id("users-header")
            .flex_none()
            .h(SECTION_HEADER_HEIGHT)
            .px(ROW_PAD_X)
            .flex()
            .items_center()
            .gap_x(px(6.))
            .rounded(ROW_RADIUS)
            .cursor_pointer()
            .text_size(px(12.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(header_glyph)
            .hover(|h| h.bg(theme.row_hover()).text_color(theme.text_main))
            .on_click(cx.listener(|this, _, _, cx| {
                this.users_collapsed = !this.users_collapsed;
                cx.notify();
            }))
            .child(
                div()
                    .flex_none()
                    .w(px(10.))
                    .child(if collapsed { "▸" } else { "▾" }),
            )
            .child(div().flex_1().child("Users"))
            .child(div().flex_none().child(count.to_string()))
            .child(self.render_help_button(theme, cx));

        let section = div()
            .px(LIST_PAD_X)
            .py(LIST_PAD_TOP)
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(theme.border_subtle)
            .child(header);

        if collapsed {
            return section.flex_none();
        }

        let own_nick = self
            .model
            .read(cx)
            .servers()
            .get(&server_id)
            .map(|s| s.nickname.clone())
            .unwrap_or_default();
        let users = Rc::new(users);
        let model = self.model.clone();
        let sidebar = cx.entity().downgrade();

        let list = uniform_list("sidebar-users", count, move |range, _, _| {
            range
                .filter_map(|ix| users.get(ix).cloned())
                .map(|user| {
                    let is_own = nick_eq(&user.nick, &own_nick);
                    let top = user.prefixes.chars().next();
                    let prefix_color = prefix_color(top, theme);
                    let nick = user.nick.clone();
                    row(
                        format!("user-{}", user.nick),
                        CHILD_ROW_HEIGHT,
                        false,
                        theme,
                    )
                    .gap_x(px(2.))
                    // 더블클릭 → DM 버퍼 열기
                    .on_click({
                        let model = model.clone();
                        let nick = nick.clone();
                        move |event: &ClickEvent, _, cx: &mut App| {
                            if event.click_count() == 2 {
                                model.update(cx, |m, cx| m.open_query(server_id, &nick, cx));
                            }
                        }
                    })
                    .on_mouse_down(MouseButton::Right, {
                        let sidebar = sidebar.clone();
                        move |event: &MouseDownEvent, _, cx: &mut App| {
                            let _ = sidebar.update(cx, |_, cx| {
                                cx.emit(SidebarEvent::UserContextMenu {
                                    server_id,
                                    nick: nick.clone(),
                                    position: event.position,
                                });
                            });
                        }
                    })
                    .child(
                        div()
                            .flex_none()
                            .w(PREFIX_WIDTH)
                            .text_color(prefix_color)
                            .children(top.map(String::from)),
                    )
                    .child(row_label(user.nick).when(is_own, |d| d.font_weight(FontWeight::MEDIUM)))
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(&self.users_scroll)
        .flex_1()
        .min_h_0();

        section.flex_1().min_h_0().child(list)
    }
}

impl Sidebar {
    /// 유저 목록 헤더 오른쪽의 도움말(?) 버튼. 누르면 권한 기호 설명 말풍선을 토글한다.
    fn render_help_button(
        &self,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let open = self.users_help_open;
        let icon_color = if open {
            theme.text_main
        } else {
            Theme::fade(theme.text_main, 0.4)
        };

        div()
            .id("users-help")
            .group("users-help")
            .relative()
            .flex_none()
            .size(HELP_BUTTON_SIZE)
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(5.))
            .cursor_pointer()
            .when(open, |d| d.bg(theme.row_hover()))
            .hover(|h| h.bg(theme.row_hover()))
            .on_hover(cx.listener(|this, hovered: &bool, _, _| {
                this.users_help_hovered = *hovered;
            }))
            // 헤더의 접기/펼치기 클릭으로 번지지 않게 막는다
            .on_click(cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.users_help_open = !this.users_help_open;
                cx.notify();
            }))
            .child(
                gpui::svg()
                    .path(ICON_CIRCLE_HELP)
                    .size(HELP_ICON_SIZE)
                    .text_color(icon_color)
                    .group_hover("users-help", |s| s.text_color(theme.text_main)),
            )
            // 말풍선 앵커: 버튼 왼쪽 위에 고정한 빈 상자 (버튼의 가운데 정렬에 영향받지 않게)
            .when(open, |d| {
                d.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .child(self.render_help_popover(theme, cx)),
                )
            })
    }

    /// 권한 기호 설명 말풍선. 버튼 위쪽에, 오른쪽 끝을 버튼에 맞춰 띄운다
    /// (유저 목록이 사이드바 아래쪽에 있으므로 위로 연다). 바깥을 누르면 닫힌다.
    fn render_help_popover(&self, theme: Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let panel = div()
            .id("users-help-popover")
            // 아래 유저 행이 hover/클릭되지 않게 막는다 (context_menu_container와 같은 이유)
            .occlude()
            .w(px(240.))
            .p(px(10.))
            .flex()
            .flex_col()
            .gap_y(px(6.))
            .rounded(px(8.))
            .border_1()
            .border_color(theme.border)
            .bg(theme.menu_bg)
            .shadow_xl()
            .text_size(px(12.))
            .font_weight(FontWeight::NORMAL)
            .text_color(theme.text_main)
            .cursor_default()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                // 버튼 위에서 누른 경우는 버튼의 토글이 처리한다
                if !this.users_help_hovered {
                    this.users_help_open = false;
                    cx.notify();
                }
            }))
            .child(div().font_weight(FontWeight::MEDIUM).child("권한 기호"))
            .children(PREFIX_HELP.iter().map(|(symbol, name, description)| {
                div()
                    .flex()
                    .items_start()
                    .gap_x(px(8.))
                    .child(
                        div()
                            .flex_none()
                            .w(PREFIX_WIDTH)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(prefix_color(symbol.chars().next(), theme))
                            .child(*symbol),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(div().font_weight(FontWeight::MEDIUM).child(*name))
                            .child(div().text_color(theme.text_subtle).child(*description)),
                    )
            }))
            .child(
                div()
                    .pt(px(2.))
                    .text_color(theme.text_subtle)
                    .child("서버마다 쓰는 기호가 다를 수 있습니다."),
            );

        gpui::deferred(
            gpui::anchored()
                .anchor(gpui::Anchor::BottomRight)
                // 이 앵커의 위치는 버튼의 왼쪽 위 — 버튼 폭만큼 오른쪽으로, 조금 위로 띄운다
                .offset(gpui::point(HELP_BUTTON_SIZE, px(-6.)))
                .snap_to_window_with_margin(px(8.))
                .child(panel),
        )
        .with_priority(1)
    }
}
