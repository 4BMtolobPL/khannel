//! ChatApp: GPUI 루트 화면. Svelte `+page.svelte`의 포팅.
//!
//! 좌측 사이드바(Sidebar) + 우측 메인(MessageView + InputBar) + 모달 3종 +
//! 컨텍스트 메뉴 2종을 조율한다. Svelte에서 `+page.svelte`가 들고 있던
//! `showServerModal`/`showChannelModal`/`showChangeNickModal`/컨텍스트 메뉴 state가
//! 이 엔티티의 필드로 이관되었다.

use std::sync::Arc;

use gpui::{
    div, prelude::*, Context, Entity, IntoElement, ParentElement, Pixels, Point, Render, Styled,
    Window,
};
use kirc_core::kirc::types::ServerId;

use crate::action::{
    Backspace, Copy, Cut, Delete, Dismiss, End, Home, Left, Paste, Right, SelectAll, SelectLeft,
    SelectRight, ShowCharacterPalette,
};
use crate::api::UiApi;
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
}

impl ChatApp {
    pub fn show(
        api: Arc<UiApi>,
        rx: crate::api::UiEventReceiver,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // TextInput 표준 편집 액션 바인딩 (gpui examples/input.rs 참고)
        cx.bind_keys([
            gpui::KeyBinding::new("backspace", Backspace, None),
            gpui::KeyBinding::new("delete", Delete, None),
            gpui::KeyBinding::new("left", Left, None),
            gpui::KeyBinding::new("right", Right, None),
            gpui::KeyBinding::new("shift-left", SelectLeft, None),
            gpui::KeyBinding::new("shift-right", SelectRight, None),
            gpui::KeyBinding::new("cmd-a", SelectAll, None),
            gpui::KeyBinding::new("ctrl-a", SelectAll, None),
            gpui::KeyBinding::new("cmd-v", Paste, None),
            gpui::KeyBinding::new("ctrl-v", Paste, None),
            gpui::KeyBinding::new("cmd-c", Copy, None),
            gpui::KeyBinding::new("ctrl-c", Copy, None),
            gpui::KeyBinding::new("cmd-x", Cut, None),
            gpui::KeyBinding::new("ctrl-x", Cut, None),
            gpui::KeyBinding::new("home", Home, None),
            gpui::KeyBinding::new("end", End, None),
            gpui::KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, None),
        ]);
        // Esc → 모달/컨텍스트 메뉴 닫기 (kirc::Dismiss)
        cx.bind_keys([gpui::KeyBinding::new("escape", Dismiss, None)]);

        // 컨텍스트 메뉴는 포커스 경로 밖 요소라 모달처럼 .on_action으로 받을 수 없다 →
        // 모달이 Dismiss를 소비하지 못한 경우(= 메뉴만 떠 있는 경우) 전역 버블 리스너로 닫는다.
        let chat = cx.entity().downgrade();
        gpui::App::on_action(cx, move |_: &Dismiss, cx| {
            chat.update(cx, |this, cx| this.close_menus(cx)).ok();
        });

        // 1) IrcModel: UiEvent 소비 → 상태 반영 (ircService.ts 포팅)
        let model = cx.new(|inner| IrcModel::start(api.clone(), rx, inner));

        // 2) 자식 뷰/모달
        let sidebar = cx.new(|cx| Sidebar::new(model.clone(), cx));
        let message_view = cx.new(|cx| MessageView::new(model.clone(), cx));
        let input_bar = cx.new(|cx| InputBar::new(api.clone(), model.clone(), window, cx));
        let server_modal = cx.new(|cx| ServerModal::new(api.clone(), model.clone(), window, cx));
        let channel_modal =
            cx.new(|cx| ChannelJoinModal::new(api.clone(), model.clone(), window, cx));
        let nick_modal = cx.new(|cx| ChangeNickModal::new(api.clone(), model.clone(), window, cx));

        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        cx.observe(&sidebar, |_, _, cx| cx.notify()).detach();
        cx.observe(&message_view, |_, _, cx| cx.notify()).detach();
        cx.observe(&input_bar, |_, _, cx| cx.notify()).detach();

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

    fn close_menus(&mut self, cx: &mut Context<Self>) {
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
                            model.update(cx, |m, cx| m.join_channel(server_id, &channel_name, cx));
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

impl Render for ChatApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::current(cx);

        // 입력창 자동 포커스 — 모달이 열려 있지 않을 때만 (모달 입력과 경쟁하지 않게)
        if !self.any_modal_open() && !self.input_bar.read(cx).input_focused(window, cx) {
            let focus = self.input_bar.read(cx).input_focus_handle(cx);
            window.focus(&focus, cx);
        }

        let server_menu = self.server_menu;
        let channel_menu = self.channel_menu.clone();

        div()
            .size_full()
            .flex()
            .bg(theme.bg_main)
            .text_color(theme.text_main)
            // ── 좌측 사이드바 ──
            .child(self.sidebar.clone())
            // ── 우측 메인 ──
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .flex_col()
                    .child(self.message_view.clone())
                    .child(self.input_bar.clone()),
            )
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
            .children(channel_menu.map(|state| self.render_channel_menu(state, cx)))
    }
}
