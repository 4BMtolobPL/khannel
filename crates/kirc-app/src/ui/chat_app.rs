//! ChatApp: GPUI 루트 화면. Svelte `+page.svelte`의 포팅.
//!
//! 좌측 사이드바(Sidebar) + 우측 메인(MessageView + InputBar) + 모달 3종 +
//! 컨텍스트 메뉴 2종을 조율한다. Svelte에서 `+page.svelte`가 들고 있던
//! `showServerModal`/`showChannelModal`/`showChangeNickModal`/컨텍스트 메뉴 state가
//! 이 엔티티의 필드로 이관되었다.

use gpui::{
    div, prelude::*, px, ClickEvent, Context, Decorations, Entity, FontWeight, IntoElement,
    MouseButton, ParentElement, Pixels, Point, Render, Styled, Window,
};
use kirc_core::kirc::types::ServerId;

use crate::model::IrcModel;
use crate::overlay::change_nick_modal::ChangeNickModal;
use crate::overlay::channel_join_modal::ChannelJoinModal;
use crate::overlay::context_menu::{
    anchored_menu, context_menu_container, context_menu_item, context_menu_separator, MenuItemStyle,
};
use crate::overlay::modal::DismissEvent;
use crate::overlay::server_modal::ServerModal;
use crate::theme::Theme;
use crate::ui::input_bar::InputBar;
use crate::ui::message_view::MessageView;
use crate::ui::sidebar::{Sidebar, SidebarEvent};
use crate::ui::window_frame::window_frame;

/// 창 맨 위의 컨트롤 줄 높이 — 신호등/사이드바 토글/닫기 버튼이 놓이는 높이이자 사이드바·메인
/// 영역의 위쪽 여백 (zed `platform_title_bar_height`와 같은 34px). 별도의 바는 그리지 않는다.
const TOP_INSET: Pixels = px(34.);
/// macOS 신호등 버튼이 차지하는 왼쪽 여백 (zed `TRAFFIC_LIGHT_PADDING` — macOS 26 기준 78px)
const TRAFFIC_LIGHT_PADDING: Pixels = px(78.);

// ==================================== 컨텍스트 메뉴 state ====================================

/// Svelte의 `serverContextMenu` state (visible/x/y/serverId → Option + position)
#[derive(Debug, Clone, Copy)]
struct ServerMenuState {
    server_id: ServerId,
    position: Point<Pixels>,
}

/// Svelte의 `channelContextMenu` state (visible/x/y/serverId/channelId/channelName)
#[derive(Debug, Clone)]
struct ChannelMenuState {
    server_id: ServerId,
    channel_id: String,
    channel_name: String,
    position: Point<Pixels>,
}

// ==================================== ChatApp ====================================

pub struct ChatApp {
    model: Entity<IrcModel>,
    sidebar: Entity<Sidebar>,
    message_view: Entity<MessageView>,
    input_bar: Entity<InputBar>,
    server_modal: Entity<ServerModal>,
    channel_modal: Entity<ChannelJoinModal>,
    nick_modal: Entity<ChangeNickModal>,

    show_server_modal: bool,
    show_channel_modal: bool,
    show_nick_modal: bool,

    server_menu: Option<ServerMenuState>,
    channel_menu: Option<ChannelMenuState>,

    /// 사이드바 표시 여부 (상단 바 토글 버튼, 저장하지 않음)
    sidebar_open: bool,
    /// 창 이동 영역(위쪽 여백)에서 마우스를 누른 상태 — 실제로 움직이면 창 이동을 시작한다
    /// (zed platform_title_bar의 should_move 패턴: 단순 클릭/더블클릭과 구분).
    title_drag_pending: bool,
}

impl ChatApp {
    /// `model`은 앱 수준에서 만들어 넘긴다 — 창을 닫았다 다시 열어도 IRC 상태가 이어진다.
    /// (키 바인딩/전역 Dismiss 리스너는 main.rs에서 앱 전체 1회 등록)
    pub fn show(model: Entity<IrcModel>, window: &Window, cx: &mut Context<Self>) -> Self {
        // 자식 뷰/모달
        let sidebar = cx.new(|cx| Sidebar::new(model.clone(), cx));
        let message_view = cx.new(|cx| MessageView::new(model.clone(), cx));
        let input_bar = cx.new(|cx| InputBar::new(model.clone(), window, cx));
        let server_modal = cx.new(|cx| ServerModal::new(model.clone(), window, cx));
        let channel_modal = cx.new(|cx| ChannelJoinModal::new(model.clone(), window, cx));
        let nick_modal = cx.new(|cx| ChangeNickModal::new(model.clone(), window, cx));

        // 자식 뷰(사이드바/메시지 뷰/입력바)는 각자 모델을 observe해 스스로 갱신한다.
        // 루트는 모델 변경 시 입력창 자동 포커스만 다시 판단하면 된다.
        cx.observe(&model, |_, _, cx| cx.notify()).detach();

        // 사이드바 이벤트 → 모달/컨텍스트 메뉴 state 갱신
        cx.subscribe_in(
            &sidebar,
            window,
            |this, _, event: &SidebarEvent, window, cx| match event {
                SidebarEvent::OpenServerModal => this.open_server_modal(window, cx),
                SidebarEvent::OpenChannelModal => this.open_channel_modal(window, cx),
                SidebarEvent::ServerContextMenu {
                    server_id,
                    position,
                } => {
                    this.server_menu = Some(ServerMenuState {
                        server_id: *server_id,
                        position: *position,
                    });
                    this.channel_menu = None;
                    cx.notify();
                }
                SidebarEvent::ChannelContextMenu {
                    server_id,
                    channel_id,
                    channel_name,
                    position,
                } => {
                    this.channel_menu = Some(ChannelMenuState {
                        server_id: *server_id,
                        channel_id: channel_id.clone(),
                        channel_name: channel_name.clone(),
                        position: *position,
                    });
                    this.server_menu = None;
                    cx.notify();
                }
            },
        )
        .detach();

        // 모달 닫힘 → 각각의 표시 플래그 해제
        // (composer 포커스는 render의 자동 포커스가 복원한다)
        cx.subscribe_in(&server_modal, window, |this, _, _: &DismissEvent, _, cx| {
            this.show_server_modal = false;
            cx.notify();
        })
        .detach();
        cx.subscribe_in(
            &channel_modal,
            window,
            |this, _, _: &DismissEvent, _, cx| {
                this.show_channel_modal = false;
                cx.notify();
            },
        )
        .detach();
        cx.subscribe_in(&nick_modal, window, |this, _, _: &DismissEvent, _, cx| {
            this.show_nick_modal = false;
            cx.notify();
        })
        .detach();

        Self {
            model,
            sidebar,
            message_view,
            input_bar,
            server_modal,
            channel_modal,
            nick_modal,
            show_server_modal: false,
            show_channel_modal: false,
            show_nick_modal: false,
            server_menu: None,
            channel_menu: None,
            sidebar_open: true,
            title_drag_pending: false,
        }
    }

    fn any_modal_open(&self) -> bool {
        self.show_server_modal || self.show_channel_modal || self.show_nick_modal
    }

    fn open_server_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_server_modal = true;
        self.server_menu = None;
        self.channel_menu = None;
        self.server_modal
            .update(cx, |modal, cx| modal.reset(window, cx));
        self.server_modal
            .update(cx, |modal, cx| modal.focus_first(window, cx));
        cx.notify();
    }

    fn open_channel_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_channel_modal = true;
        self.server_menu = None;
        self.channel_menu = None;
        self.channel_modal
            .update(cx, |modal, cx| modal.reset(window, cx));
        self.channel_modal
            .update(cx, |modal, cx| modal.focus_first(window, cx));
        cx.notify();
    }

    fn open_nick_modal(
        &mut self,
        server_id: ServerId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_nick_modal = true;
        self.server_menu = None;
        self.channel_menu = None;
        self.nick_modal
            .update(cx, |modal, cx| modal.open_for(Some(server_id), window, cx));
        self.nick_modal
            .update(cx, |modal, cx| modal.focus_first(window, cx));
        cx.notify();
    }

    pub fn close_menus(&mut self, cx: &mut Context<Self>) {
        if self.server_menu.is_some() || self.channel_menu.is_some() {
            self.server_menu = None;
            self.channel_menu = None;
            cx.notify();
        }
    }

    // ── 서버 컨텍스트 메뉴 (Svelte: Connect / Disconnect / Copy Server Name / Change Nickname) ──
    //
    // 항목 클릭 시 메뉴를 닫는다 (Svelte: window click listener가 메뉴 안 클릭에도 닫았다).
    // 모델 액션은 메뉴를 닫은 뒤 실행한다.

    fn render_server_menu(
        &mut self,
        state: ServerMenuState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = Theme::current(cx);
        let model = self.model.clone();
        let chat = cx.entity();
        let id = state.server_id;

        let menu = context_menu_container(theme)
            // 메뉴 밖 클릭 → 닫기 (Svelte: window click listener 대체)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_menus(cx)))
            // Connect — Svelte는 핸들러 없는 데코레이션 버튼이지만, 재연결 로직이
            // IrcModel에 이미 있으므로 연결해 둔다.
            .child(
                context_menu_item(
                    "menu-connect",
                    "🔌",
                    "Connect",
                    MenuItemStyle::Accent,
                    theme,
                )
                .on_click({
                    let chat = chat.clone();
                    let model = model.clone();
                    move |_, _, cx| {
                        chat.update(cx, |this, cx| this.close_menus(cx));
                        model.update(cx, |m, cx| m.connect_server(id, cx));
                    }
                }),
            )
            .child(
                context_menu_item(
                    "menu-disconnect",
                    "🚫",
                    "Disconnect",
                    MenuItemStyle::Danger,
                    theme,
                )
                .on_click({
                    let chat = chat.clone();
                    let model = model.clone();
                    move |_, _, cx| {
                        chat.update(cx, |this, cx| this.close_menus(cx));
                        model.update(cx, |m, cx| m.disconnect_server(id, cx));
                    }
                }),
            )
            .child(context_menu_separator(theme))
            .child(
                context_menu_item(
                    "menu-copy-server",
                    "📋",
                    "Copy Server Name",
                    MenuItemStyle::Normal,
                    theme,
                )
                .on_click({
                    let chat = chat.clone();
                    let model = model.clone();
                    move |_, _, cx| {
                        chat.update(cx, |this, cx| this.close_menus(cx));
                        let name = model
                            .read(cx)
                            .servers()
                            .get(&id)
                            .map(|server| server.name.clone());
                        if let Some(name) = name {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(name));
                        }
                    }
                }),
            )
            .child(
                context_menu_item(
                    "menu-change-nick",
                    "👤",
                    "Change Nickname",
                    MenuItemStyle::Normal,
                    theme,
                )
                .on_click({
                    let chat = chat.clone();
                    // open_nick_modal이 메뉴도 닫는다
                    move |_, window, cx| {
                        chat.update(cx, |this, cx| this.open_nick_modal(id, window, cx));
                    }
                }),
            );

        anchored_menu(state.position, menu)
    }

    // ── 채널 컨텍스트 메뉴 (Svelte: Join / Leave / Copy Channel Name) ──

    fn render_channel_menu(
        &mut self,
        state: ChannelMenuState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = Theme::current(cx);
        let model = self.model.clone();
        let chat = cx.entity();

        let menu = context_menu_container(theme)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_menus(cx)))
            .child(
                context_menu_item("menu-join", "📥", "Join", MenuItemStyle::Accent, theme)
                    .on_click({
                        let chat = chat.clone();
                        let model = model.clone();
                        let server_id = state.server_id;
                        let channel_name = state.channel_name.clone();
                        move |_, _, cx| {
                            chat.update(cx, |this, cx| this.close_menus(cx));
                            // 실패는 model에서 로깅된다 (메뉴엔 표시 영역이 없음)
                            let _ = model
                                .update(cx, |m, cx| m.join_channel(server_id, &channel_name, cx));
                        }
                    }),
            )
            .child(
                context_menu_item("menu-leave", "🚪", "Leave", MenuItemStyle::Danger, theme)
                    .on_click({
                        let chat = chat.clone();
                        let model = model.clone();
                        let channel_id = state.channel_id.clone();
                        move |_, _, cx| {
                            chat.update(cx, |this, cx| this.close_menus(cx));
                            model.update(cx, |m, cx| m.leave_channel(&channel_id, cx));
                        }
                    }),
            )
            .child(context_menu_separator(theme))
            .child(
                context_menu_item(
                    "menu-copy-channel",
                    "📋",
                    "Copy Channel Name",
                    MenuItemStyle::Normal,
                    theme,
                )
                .on_click({
                    let chat = chat.clone();
                    let name = state.channel_name.clone();
                    move |_, _, cx| {
                        chat.update(cx, |this, cx| this.close_menus(cx));
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(name.clone()));
                    }
                }),
            );

        anchored_menu(state.position, menu)
    }
}

impl ChatApp {
    fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_open = !self.sidebar_open;
        cx.notify();
    }

    /// 창 이동 영역: 누른 채 움직이면 창 이동, 더블클릭이면 확대/축소.
    /// 사이드바·메인 영역의 위쪽 여백에 쓰인다 (시스템 타이틀바 대체).
    fn drag_region(&self, id: &'static str, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let is_macos = cfg!(target_os = "macos");
        div()
            .id(id)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.title_drag_pending = true),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.title_drag_pending = false),
            )
            .on_mouse_down_out(cx.listener(|this, _, _, _| this.title_drag_pending = false))
            .on_mouse_move(cx.listener(|this, _, window, _| {
                if this.title_drag_pending {
                    this.title_drag_pending = false;
                    window.start_window_move();
                }
            }))
            .on_click(move |event: &ClickEvent, window, _| {
                if event.click_count() == 2 {
                    if is_macos {
                        // 시스템 설정(제목 막대 더블클릭 동작)을 따른다
                        window.titlebar_double_click();
                    } else {
                        window.zoom_window();
                    }
                }
            })
    }

    /// 창 왼쪽 위에 고정된 컨트롤: (macOS 신호등 자리) + 사이드바 토글.
    /// 사이드바를 열고 닫아도 같은 자리에 머문다 — 열리면 그 아래로 서버/채널 목록이 보인다.
    fn render_corner_controls(&self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let theme = Theme::current(cx);
        let left = if cfg!(target_os = "macos") && !window.is_fullscreen() {
            TRAFFIC_LIGHT_PADDING
        } else {
            px(8.)
        };

        div()
            .absolute()
            .top_0()
            .left_0()
            .h(TOP_INSET)
            .pl(left)
            .flex()
            .items_center()
            .child(
                window_button("toggle-sidebar", "☰", theme)
                    .hover(|h| h.bg(theme.hover).text_color(theme.text_main))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_sidebar(cx))),
            )
    }

    /// 메인 영역 위쪽 여백: 창 이동 영역 + (Linux CSD일 때) 오른쪽 위 닫기 버튼.
    fn render_main_top(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::current(cx);
        // Linux에서 컴포지터가 CSD를 받아들였을 때만 닫기 버튼을 그린다
        // (서버 측 장식이면 시스템 타이틀바에 이미 닫기 버튼이 있다).
        let show_close = !cfg!(target_os = "macos")
            && matches!(window.window_decorations(), Decorations::Client { .. });

        div()
            .flex_shrink_0()
            .h(TOP_INSET)
            .w_full()
            .flex()
            .items_center()
            .pr(px(8.))
            .child(self.drag_region("main-drag", cx).flex_1().h_full())
            .when(show_close, |d| {
                d.child(
                    window_button("window-close", "✕", theme)
                        .font_weight(FontWeight::MEDIUM)
                        .hover(|h| h.bg(theme.danger_hover_bg).text_color(theme.danger_text))
                        // Linux는 마지막 창을 닫으면 앱이 종료된다(on_app_quit → graceful shutdown)
                        .on_click(|_, window, _| window.remove_window()),
                )
            })
    }
}

/// 창 컨트롤 버튼 공통 모양 (사이드바 토글, 닫기)
fn window_button(id: &'static str, label: &'static str, theme: Theme) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .w(px(28.))
        .h(px(24.))
        .rounded(px(4.))
        .cursor_pointer()
        .text_color(theme.text_subtle)
        .child(label)
}

impl Render for ChatApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::current(cx);

        // 입력창 자동 포커스 — 모달이 열려 있지 않을 때만 (모달 입력과 경쟁하지 않게).
        // 렌더 중 포커스를 바꾸지 않고 렌더 이후로 미룬다(zed thread_view.rs 패턴).
        // 미룬 시점에 조건을 다시 확인한다 — 그 사이 모달이 열렸을 수 있다.
        if !self.any_modal_open() && !self.input_bar.read(cx).input_focused(window, cx) {
            cx.defer_in(window, |this, window, cx| {
                if this.any_modal_open() || this.input_bar.read(cx).input_focused(window, cx) {
                    return;
                }
                let focus = this.input_bar.read(cx).input_focus_handle(cx);
                window.focus(&focus, cx);
            });
        }

        let server_menu = self.server_menu;
        let channel_menu = self.channel_menu.clone();

        let main_top = self.render_main_top(window, cx);
        let corner_controls = self.render_corner_controls(window, cx);

        let content = div()
            .relative()
            .size_full()
            .flex()
            .bg(theme.bg_main)
            .text_color(theme.text_main)
            // ── 좌측 사이드바 (토글): 위쪽 여백(창 이동 영역, 토글 버튼 자리) + 서버/채널 목록 ──
            .when(self.sidebar_open, |d| {
                d.child(
                    div()
                        .flex_shrink_0()
                        .flex()
                        .flex_col()
                        .child(
                            self.drag_region("sidebar-drag", cx)
                                .flex_shrink_0()
                                .h(TOP_INSET)
                                .bg(theme.bg_side)
                                .border_r_1()
                                .border_color(theme.border),
                        )
                        .child(self.sidebar.clone()),
                )
            })
            // ── 우측 메인: 위쪽 여백(창 이동 영역 + Linux 닫기 버튼) + 메시지 + 입력 ──
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .flex_col()
                    .child(main_top)
                    .child(self.message_view.clone())
                    .child(self.input_bar.clone()),
            )
            // ── 왼쪽 위 고정 컨트롤 (사이드바 토글) — 사이드바/메인 위에 얹는다 ──
            .child(corner_controls)
            // ── 모달 ──
            .when(self.show_channel_modal, |d| {
                d.child(self.channel_modal.clone())
            })
            .when(self.show_server_modal, |d| {
                d.child(self.server_modal.clone())
            })
            .when(self.show_nick_modal, |d| d.child(self.nick_modal.clone()))
            // ── 컨텍스트 메뉴 ──
            .children(server_menu.map(|state| self.render_server_menu(state, cx)))
            .children(channel_menu.map(|state| self.render_channel_menu(state, cx)));

        // Linux CSD: 그림자/테두리/가장자리 리사이즈 (macOS는 그대로 통과)
        window_frame(content, window, cx)
    }
}
