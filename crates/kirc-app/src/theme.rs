//! UI 색상 팔레트 — Svelte의 Tailwind 유틸리티 클래스를 GPUI 스타일로 매핑.
//!
//! Svelte 앱은 Tailwind `dark:` 변형(prefers-color-scheme)으로 다크 모드를 지원하므로,
//! GPUI에서는 [`Theme::current`]가 `cx.window_appearance()`로 동일한 기준으로 선택한다.
//! (MIGRATION.md Phase 3 표: "Tailwind class → GPUI 스타일 (dark mode: ActiveTheme 환경 or 자체 toggle)")

use gpui::{App, Rgba, WindowAppearance, rgba, rgb};

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    // ── 배경 ──
    /// 메인 영역 (light: neutral-100 / dark: neutral-900)
    pub bg_main: Rgba,
    /// 사이드바 (light: neutral-50 / dark: neutral-900)
    pub bg_side: Rgba,
    /// 텍스트 입력 상자 (light: white / dark: neutral-800)
    pub bg_input: Rgba,
    /// 모달 패널 (light: white / dark: neutral-900)
    pub panel: Rgba,
    // ── 보더 ──
    /// 주요 경계선 (light: neutral-300 / dark: neutral-700)
    pub border: Rgba,
    /// 옅은 구분선 (메뉴 separator 등, light: neutral-100 / dark: neutral-700)
    pub border_subtle: Rgba,
    // ── 텍스트 ──
    pub text_main: Rgba,
    pub text_subtle: Rgba,
    /// 시스템 메시지/라벨 (Svelte: text-gray-500, 양쪽 모드 동일)
    pub text_system: Rgba,
    // ── 인터랙션 ──
    pub hover: Rgba,
    /// 서버 row 선택 (light: neutral-200 / dark: neutral-700)
    pub selected: Rgba,
    /// 채널 row 선택 (서버보다 한 단계 진하게, Svelte: neutral-300 / neutral-600)
    pub selected_channel: Rgba,
    // ── 액센트 (sky) ──
    pub accent: Rgba,
    pub accent_hover: Rgba,
    /// 닉네임 강조 텍스트 (light: sky-600 / dark: sky-400)
    pub accent_text: Rgba,
    /// 컨텍스트 메뉴 accent hover 배경 (sky-50 / sky-900 alpha 30%)
    pub accent_hover_bg: Rgba,
    /// 입력 포커스 링 (sky-500)
    pub focus_ring: Rgba,
    // ── 배지/에러 ──
    pub badge_bg: Rgba,
    pub badge_text: Rgba,
    /// 액센트/위험 배경 위의 텍스트 (흰색)
    pub text_inverse: Rgba,
    /// 에러 텍스트 (Svelte: text-red-500, 양쪽 모드 동일)
    pub error_text: Rgba,
    /// 컨텍스트 메뉴 danger hover 배경 (red-50 / red-900 alpha 30%)
    pub danger_hover_bg: Rgba,
    /// 컨텍스트 메뉴 danger hover 텍스트 (red-600 / red-400)
    pub danger_text: Rgba,
    // ── 상태 점 ──
    pub status_connected: Rgba,
    pub status_connecting: Rgba,
    pub status_disconnected: Rgba,
    // ── 오버레이 ──
    /// 모달 배경 스크림 (black/30)
    pub overlay_scrim: Rgba,
    /// 컨텍스트 메뉴 배경 (light: white/95 / dark: neutral-800/95)
    pub menu_bg: Rgba,
    /// 컨텍스트 메뉴 일반 hover 배경 (light: neutral-100 / dark: neutral-700)
    pub menu_hover: Rgba,
}

impl Theme {
    /// `prefers-color-scheme`에 해당하는 윈도우 외관으로 라이트/다크를 결정한다.
    pub fn current(cx: &App) -> Theme {
        match cx.window_appearance() {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::dark(),
            _ => Self::light(),
        }
    }

    pub fn light() -> Theme {
        let neutral_50 = rgb(0xfafafa);
        let neutral_100 = rgb(0xf5f5f5);
        let neutral_200 = rgb(0xe5e5e5);
        let neutral_300 = rgb(0xd4d4d4);
        let neutral_500 = rgb(0x737373);
        let neutral_900 = rgb(0x171717);
        let gray_500 = rgb(0x6b7280);
        let sky_50 = rgb(0xf0f9ff);
        let sky_500 = rgb(0x0ea5e9);
        let sky_600 = rgb(0x0284c7);
        let red_50 = rgb(0xfef2f2);
        let red_500 = rgb(0xef4444);
        let red_600 = rgb(0xdc2626);
        let green_500 = rgb(0x22c55e);
        let yellow_500 = rgb(0xeab308);
        let white = rgb(0xffffff);

        Theme {
            bg_main: neutral_100,
            bg_side: neutral_50,
            bg_input: white,
            panel: white,
            border: neutral_300,
            border_subtle: neutral_100,
            text_main: neutral_900,
            text_subtle: neutral_500,
            text_system: gray_500,
            hover: neutral_100,
            selected: neutral_200,
            selected_channel: neutral_300,
            accent: sky_600,
            accent_hover: sky_500,
            accent_text: sky_600,
            accent_hover_bg: sky_50,
            focus_ring: sky_500,
            badge_bg: red_500,
            badge_text: white,
            text_inverse: white,
            error_text: red_500,
            danger_hover_bg: red_50,
            danger_text: red_600,
            status_connected: green_500,
            status_connecting: yellow_500,
            status_disconnected: red_500,
            overlay_scrim: rgba(0x0000004d), // black/30
            menu_bg: rgba(0xfffffff2),       // white/95
            menu_hover: neutral_100,
        }
    }

    pub fn dark() -> Theme {
        let neutral_100 = rgb(0xf5f5f5);
        let neutral_400 = rgb(0xa3a3a3);
        let neutral_600 = rgb(0x525252);
        let neutral_700 = rgb(0x404040);
        let neutral_800 = rgb(0x262626);
        let neutral_900 = rgb(0x171717);
        let gray_500 = rgb(0x6b7280);
        let sky_400 = rgb(0x38bdf8);
        let sky_500 = rgb(0x0ea5e9);
        let sky_600 = rgb(0x0284c7);
        let red_400 = rgb(0xf87171);
        let red_500 = rgb(0xef4444);
        let green_500 = rgb(0x22c55e);
        let yellow_500 = rgb(0xeab308);

        Theme {
            bg_main: neutral_900,
            bg_side: neutral_900,
            bg_input: neutral_800,
            panel: neutral_900,
            border: neutral_700,
            border_subtle: neutral_700,
            text_main: neutral_100,
            text_subtle: neutral_400,
            text_system: gray_500,
            hover: neutral_800,
            selected: neutral_700,
            selected_channel: neutral_600,
            accent: sky_600,
            accent_hover: sky_500,
            accent_text: sky_400,
            accent_hover_bg: rgba(0x0c4a6e4d), // sky-900/30
            focus_ring: sky_500,
            badge_bg: red_500,
            badge_text: rgb(0xffffff),
            text_inverse: rgb(0xffffff),
            error_text: red_500,
            danger_hover_bg: rgba(0x7f1d1f4d), // red-900/30
            danger_text: red_400,
            status_connected: green_500,
            status_connecting: yellow_500,
            status_disconnected: red_500,
            overlay_scrim: rgba(0x0000004d),   // black/30
            menu_bg: rgba(0x262626f2),         // neutral-800/95
            menu_hover: neutral_700,
        }
    }
}

/// 상태 점 색상 (Svelte: connected → green-500, connecting/registering → yellow-500, else red-500)
pub fn status_dot_color(theme: &Theme, is_connected: bool, is_connecting: bool) -> Rgba {
    if is_connected {
        theme.status_connected
    } else if is_connecting {
        theme.status_connecting
    } else {
        theme.status_disconnected
    }
}
