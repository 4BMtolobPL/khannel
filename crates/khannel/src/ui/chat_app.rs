//! ChatApp: GPUI 루트 화면. Svelte `+page.svelte`의 포팅.
//!
//! 좌측 사이드바(Sidebar) + 우측 메인(MessageView + InputBar) + 모달 3종 +
//! 컨텍스트 메뉴 2종을 조율한다. Svelte에서 `+page.svelte`가 들고 있던
//! `showServerModal`/`showChannelModal`/`showChangeNickModal`/컨텍스트 메뉴 state가
//! 이 엔티티의 필드로 이관되었다.

use gpui::{
    ClickEvent, Context, Decorations, Entity, FontWeight, IntoElement, MouseButton, MouseMoveEvent,
    ParentElement, Pixels, Point, Render, Styled, Window, WindowControlArea, div, prelude::*, px,
};
use khannel_core::client::types::ServerId;

use crate::model::IrcModel;
use crate::overlay::change_nick_modal::ChangeNickModal;
use crate::overlay::channel_join_modal::ChannelJoinModal;
use crate::overlay::context_menu::{
    MenuItemStyle, anchored_menu, context_menu_container, context_menu_item, context_menu_separator,
};
use crate::overlay::modal::DismissEvent;
use crate::overlay::remove_server_modal::RemoveServerModal;
use crate::overlay::server_modal::ServerModal;
use crate::theme::Theme;
use crate::ui::input_bar::InputBar;
use crate::ui::message_view::MessageView;
use crate::ui::sidebar::{Sidebar, SidebarEvent};
use crate::ui::window_frame::window_frame;

// ── 창 컨트롤 줄 기하 (zeron shell.rs / theme.rs 수치) ──
/// 창 맨 위의 컨트롤 줄 높이 — 신호등/사이드바 토글/닫기 버튼이 놓이는 높이이자 사이드바·메인
/// 영역의 위쪽 여백 (zeron `Theme::TITLEBAR_HEIGHT`). 별도의 바는 그리지 않는다.
pub const TOP_INSET: Pixels = px(38.);
/// 컨트롤을 줄 중앙보다 아래로 내리는 위쪽 패딩 (zeron `TITLEBAR_TOP_PAD`).
/// (38 + 4) / 2 = 21 → macOS 신호등(위치 y=14, 높이 14) 중심과 일치한다.
const TOP_PAD: Pixels = px(4.);
/// 컨트롤 묶음의 좌우 패딩 (zeron `TITLEBAR_CLUSTER_PAD`)
const CLUSTER_PAD: Pixels = px(10.);
/// macOS에서 컨트롤 묶음이 시작하는 x (zeron `titlebar_cluster_start`: 신호등 오른쪽 88px,
/// 전체 화면이면 신호등이 숨어 12px)
const MAC_CLUSTER_START: f32 = 88.;
const MAC_CLUSTER_START_FULLSCREEN: f32 = 12.;
/// macOS 신호등 버튼의 x 위치 (main.rs `traffic_light_position`과 같은 값)
const MAC_TRAFFIC_LIGHT_X: f32 = 14.;
/// 창 컨트롤 버튼 크기 / 모서리 (zeron `window_control_button`: 24px, r6)
const CONTROL_SIZE: Pixels = px(24.);
const CONTROL_RADIUS: Pixels = px(6.);
/// island(버튼 묶음 배경) 높이 / 모서리 / 왼쪽 시작 (zeron `titlebar_island_vertical_geometry`)
const ISLAND_HEIGHT: f32 = 32.;
const ISLAND_RADIUS: Pixels = px(12.);
const ISLAND_LEFT: Pixels = px(6.);

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
    remove_modal: Entity<RemoveServerModal>,

    show_server_modal: bool,
    show_channel_modal: bool,
    show_nick_modal: bool,
    show_remove_modal: bool,

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
        let remove_modal = cx.new(|cx| RemoveServerModal::new(model.clone(), cx));

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
        cx.subscribe_in(&remove_modal, window, |this, _, _: &DismissEvent, _, cx| {
            this.show_remove_modal = false;
            cx.notify();
        })
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
            remove_modal,
            show_server_modal: false,
            show_channel_modal: false,
            show_nick_modal: false,
            show_remove_modal: false,
            server_menu: None,
            channel_menu: None,
            sidebar_open: true,
            title_drag_pending: false,
        }
    }

    fn any_modal_open(&self) -> bool {
        self.show_server_modal
            || self.show_channel_modal
            || self.show_nick_modal
            || self.show_remove_modal
    }

    fn open_remove_modal(
        &mut self,
        server_id: ServerId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_remove_modal = true;
        self.server_menu = None;
        self.channel_menu = None;
        self.remove_modal.update(cx, |modal, cx| {
            modal.open_for(server_id, cx);
            modal.focus(window, cx);
        });
        cx.notify();
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

    // ── 서버 컨텍스트 메뉴 (Svelte: Connect / Disconnect / Copy Server Name / Change Nickname,
    //    + Delete Server — khannel 추가) ──
    //
    // 항목 클릭 시 메뉴를 닫는다 (Svelte: window click listener가 메뉴 안 클릭에도 닫았다).
    // 모델 액션은 메뉴를 닫은 뒤 실행한다.

    fn render_server_menu(
        &mut self,
        state: ServerMenuState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
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
            )
            .child(context_menu_separator(theme))
            // 서버 삭제 — 되돌릴 수 없어 확인 모달을 거친다
            .child(
                context_menu_item(
                    "menu-remove-server",
                    "🗑",
                    "Delete Server",
                    MenuItemStyle::Danger,
                    theme,
                )
                .on_click({
                    let chat = chat.clone();
                    // open_remove_modal이 메뉴도 닫는다
                    move |_, window, cx| {
                        chat.update(cx, |this, cx| this.open_remove_modal(id, window, cx));
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
    ) -> impl IntoElement + use<> {
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
    /// 사이드바·메인 영역의 위쪽 여백에 쓰인다 (시스템 타이틀바 대체, zeron `titlebar_drag_region`).
    fn drag_region(&self, id: &'static str, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let is_macos = cfg!(target_os = "macos");
        div()
            .id(id)
            // macOS: app-owned titlebar의 드래그 영역으로 표시 (WindowOptions.app_owns_titlebar_drag)
            .window_control_area(WindowControlArea::Drag)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.title_drag_pending = true),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.title_drag_pending = false),
            )
            .on_mouse_down_out(cx.listener(|this, _, _, _| this.title_drag_pending = false))
            // 버튼이 실제로 눌린 채 움직일 때만 창 이동을 시작한다. macOS의 네이티브 드래그 세션은
            // 그 안의 빠른 두 번째 클릭을 제목 막대 더블클릭(확대)으로 처리하므로, 남아 있던
            // pending 상태만으로 hover 이동 중에 세션을 열면 안 된다 (zeron 주석 참고).
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, _| {
                if this.title_drag_pending && event.pressed_button == Some(MouseButton::Left) {
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

    /// 창 왼쪽 위에 고정된 컨트롤 묶음(island): [macOS 신호등] + 사이드바 토글.
    ///
    /// zeron `render_titlebar_cluster`: 창 왼쪽 위에 얹힌 오버레이라 사이드바를 열고 닫아도 같은
    /// 자리에 머문다. 컨테이너 자체는 리스너가 없어 버튼 사이 빈 곳은 아래 창 이동 영역으로
    /// 통과하고, 버튼은 `occlude`로 아래로 이벤트를 넘기지 않는다. island 배경은 x=6부터
    /// 컨트롤 끝까지 걸쳐 macOS 신호등까지 하나의 묶음으로 감싼다.
    fn render_corner_controls(&self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let theme = Theme::current(cx);
        let cluster_start = if !cfg!(target_os = "macos") {
            0.
        } else if window.is_fullscreen() {
            MAC_CLUSTER_START_FULLSCREEN
        } else {
            MAC_CLUSTER_START
        };
        // 컨테이너 자체 패딩을 뺀 만큼 신호등 자리를 비운다 (zeron `titlebar_spacer_width`)
        let spacer = (cluster_start - f32::from(CLUSTER_PAD)).max(0.);
        // island를 패딩 포함 줄의 중심(= 신호등 중심)에 맞춘다
        let island_top = (f32::from(TOP_INSET) + f32::from(TOP_PAD)) / 2. - ISLAND_HEIGHT / 2.;
        // island 좌우 여백을 같게: 왼쪽 여백(island 시작 → 첫 항목)만큼 마지막 버튼 오른쪽에도 둔다.
        // 첫 항목은 macOS(창 모드)면 신호등, 아니면 토글 버튼(컨테이너 패딩 위치).
        let first_item_x = if cfg!(target_os = "macos") && !window.is_fullscreen() {
            MAC_TRAFFIC_LIGHT_X
        } else {
            f32::from(CLUSTER_PAD).max(cluster_start)
        };
        let side_gap = first_item_x - f32::from(ISLAND_LEFT);
        // island 오른쪽 끝 = 컨테이너 오른쪽 끝(마지막 버튼 + 패딩)에서 (패딩 - 여백)만큼 안쪽
        let island_right = f32::from(CLUSTER_PAD) - side_gap;

        div()
            .absolute()
            .top_0()
            .left_0()
            .h(TOP_INSET)
            .pt(TOP_PAD)
            .px(CLUSTER_PAD)
            .flex()
            .items_center()
            // island 배경 (버튼 뒤)
            .child(
                div()
                    .absolute()
                    .left(ISLAND_LEFT)
                    .right(px(island_right))
                    .top(px(island_top))
                    .h(px(ISLAND_HEIGHT))
                    .rounded(ISLAND_RADIUS)
                    .bg(theme.island_bg())
                    .border_1()
                    .border_color(theme.border_subtle)
                    .shadow_sm(),
            )
            .when(spacer > 0., |d| {
                d.child(div().flex_none().h_full().w(px(spacer)))
            })
            .child(
                window_control_button("toggle-sidebar", hamburger_icon(theme), theme)
                    .hover(|h| h.bg(theme.row_hover()))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        this.toggle_sidebar(cx)
                    })),
            )
    }

    /// Linux CSD일 때 창 오른쪽 위의 닫기 버튼 (zeron `render_linux_caption_controls`).
    /// 서버 측 장식이면 시스템 타이틀바에 이미 닫기 버튼이 있으므로 그리지 않는다.
    fn render_close_control(&self, window: &Window, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let show_close = !cfg!(target_os = "macos")
            && matches!(window.window_decorations(), Decorations::Client { .. });
        if !show_close {
            return None;
        }
        let theme = Theme::current(cx);
        Some(
            div()
                .absolute()
                .top_0()
                .right_0()
                .h(TOP_INSET)
                .pt(TOP_PAD)
                .px(CLUSTER_PAD)
                .flex()
                .items_center()
                .child(
                    window_control_button("window-close", "✕", theme)
                        .font_weight(FontWeight::MEDIUM)
                        // zeron linux_caption_button: 닫기는 빨간 배경 + 흰 글자
                        .hover(|h| h.bg(gpui::rgb(0xe81123)).text_color(gpui::white()))
                        // Linux는 마지막 창을 닫으면 앱이 종료된다(on_app_quit → graceful shutdown)
                        .on_click(|_, window, cx| {
                            cx.stop_propagation();
                            window.remove_window()
                        }),
                ),
        )
    }
}

/// 창 컨트롤 버튼 공통 모양 (사이드바 토글, 닫기) — zeron `window_control_button`.
///
/// 창 이동 영역 위에 놓이므로 아래로 마우스 이벤트가 전혀 넘어가지 않아야 한다.
/// `occlude`로 hit test를 버튼에서 멈춰, 아래 영역의 드래그 시작·더블클릭 확대(is_hovered 기반)가
/// 버튼 위에서는 발화하지 않게 한다. (이전: 토글을 빠르게 두 번 누르면 창이 최대화됐다)
fn window_control_button(
    id: &'static str,
    glyph: impl IntoElement,
    theme: Theme,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .group(id)
        .size(CONTROL_SIZE)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(CONTROL_RADIUS)
        .cursor_pointer()
        .text_color(theme.text_subtle)
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
        .child(glyph)
}

/// 사이드바 토글 아이콘(☰)을 막대 3개로 직접 그린다.
///
/// `☰` 글자는 macOS에서 대체 폰트로 그려지는데, 그 폰트의 글자 높이 정보 때문에 버튼 상자를
/// 가운데 정렬해도 그림이 좌하단으로 쏠렸다. div로 그리면 폰트와 무관하게 정확히 가운데에 온다.
/// 버튼 hover 시 막대 색은 group hover로 바꾼다(div 배경은 글자색을 상속하지 않는다).
fn hamburger_icon(theme: Theme) -> gpui::Div {
    let bar = || {
        div()
            .w_full()
            .h(px(1.5))
            .rounded_full()
            .bg(theme.text_subtle)
            .group_hover("toggle-sidebar", |s| s.bg(theme.text_main))
    };
    div()
        .w(px(14.))
        .flex()
        .flex_col()
        .gap(px(3.))
        .child(bar())
        .child(bar())
        .child(bar())
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

        let corner_controls = self.render_corner_controls(window, cx);
        let close_control = self.render_close_control(window, cx);

        let content = div()
            .relative()
            .size_full()
            .flex()
            .bg(theme.bg_main)
            .text_color(theme.text_main)
            // ── 좌측 사이드바 (토글): 창 전체 높이의 톤 + 오른쪽 hairline (zeron sidebar tone).
            //    위쪽 여백(창 이동 영역, 컨트롤 자리) 아래로 서버/채널 목록 ──
            .when(self.sidebar_open, |d| {
                d.child(
                    div()
                        .flex_shrink_0()
                        .flex()
                        .flex_col()
                        .bg(theme.sidebar_tone())
                        .border_r_1()
                        .border_color(theme.border)
                        .child(
                            self.drag_region("sidebar-drag", cx)
                                .flex_shrink_0()
                                .h(TOP_INSET),
                        )
                        .child(self.sidebar.clone()),
                )
            })
            // ── 우측 메인: 위쪽 여백(창 이동 영역) + 메시지 + 입력 ──
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .flex_col()
                    .child(
                        self.drag_region("main-drag", cx)
                            .flex_shrink_0()
                            .h(TOP_INSET),
                    )
                    .child(self.message_view.clone())
                    .child(self.input_bar.clone()),
            )
            // ── 창 컨트롤 (사이드바/메인 위에 얹는 오버레이) ──
            .child(corner_controls)
            .children(close_control)
            // ── 모달 ──
            .when(self.show_channel_modal, |d| {
                d.child(self.channel_modal.clone())
            })
            .when(self.show_server_modal, |d| {
                d.child(self.server_modal.clone())
            })
            .when(self.show_nick_modal, |d| d.child(self.nick_modal.clone()))
            .when(self.show_remove_modal, |d| {
                d.child(self.remove_modal.clone())
            })
            // ── 컨텍스트 메뉴 ──
            .children(server_menu.map(|state| self.render_server_menu(state, cx)))
            .children(channel_menu.map(|state| self.render_channel_menu(state, cx)));

        // Linux CSD: 그림자/테두리/가장자리 리사이즈 (macOS는 그대로 통과)
        window_frame(content, window, cx)
    }
}
