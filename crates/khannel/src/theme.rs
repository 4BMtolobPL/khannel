//! UI 색상 팔레트 — **One Light (라이트) / One Dark (다크)**.
//!
//! 기존 Tailwind neutral/sky 매핑을 Atom Color Scheme 기본 팔레트로 교체했다:
//! - 라이트: One Light (`#fafafa`, text `#282c34`, accent blue `#4078f2` ...)
//! - 다크:   One Dark  (`#282c34`, text `#abb2bf`, accent blue `#61afef` ...)
//!
//! 반투명 색상 규칙: 배경/서페이스는 기본 **CC**(80%), 겹치는 인터랙션 표면
//! (hover/선택/hover 배경/스크림)는 **80**(50%). border·텍스트·작은 요소
//! (badge·상태 점·focus ring)는 투명도 없이 불투명.
//!
//! Svelte 앱은 Tailwind `dark:` 변형(prefers-color-scheme)으로 다크 모드를 지원하므로,
//! GPUI에서는 [`Theme::current`]가 `cx.window_appearance()`로 동일한 기준으로 선택한다.
//! (docs/MIGRATION.md Phase 3 표: "Tailwind class → GPUI 스타일 (dark mode: ActiveTheme 환경 or 자체 toggle)")

use gpui::{App, Rgba, WindowAppearance, rgb, rgba};

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    // ── 배경 ──
    /// 메인 영역
    pub bg_main: Rgba,
    /// 텍스트 입력 상자
    pub bg_input: Rgba,
    /// 모달 패널
    pub panel: Rgba,
    // ── 보더 ──
    /// 주요 경계선
    pub border: Rgba,
    /// 옅은 구분선 (메뉴 separator 등)
    pub border_subtle: Rgba,
    // ── 텍스트 ──
    pub text_main: Rgba,
    pub text_subtle: Rgba,
    /// 시스템 메시지/라벨 (양쪽 모드 동일)
    pub text_system: Rgba,
    // ── 인터랙션 ──
    pub hover: Rgba,
    // ── 액센트 ──
    pub accent: Rgba,
    pub accent_hover: Rgba,
    /// 닉네임 강조 텍스트
    pub accent_text: Rgba,
    /// 컨텍스트 메뉴 accent hover 배경
    pub accent_hover_bg: Rgba,
    /// 입력 포커스 링
    pub focus_ring: Rgba,
    // ── 배지/에러 ──
    pub badge_bg: Rgba,
    pub badge_text: Rgba,
    /// 액센트/위험 배경 위의 텍스트 (흰색)
    pub text_inverse: Rgba,
    /// 에러 텍스트 (양쪽 모드 동일)
    pub error_text: Rgba,
    /// 컨텍스트 메뉴 danger hover 배경
    pub danger_hover_bg: Rgba,
    /// 컨텍스트 메뉴 danger hover 텍스트
    pub danger_text: Rgba,
    // ── 상태 점 ──
    pub status_connected: Rgba,
    pub status_connecting: Rgba,
    pub status_disconnected: Rgba,
    // ── 오버레이 ──
    /// 모달 배경 스크림 (black/30)
    pub overlay_scrim: Rgba,
    /// 컨텍스트 메뉴 배경
    pub menu_bg: Rgba,
    /// 컨텍스트 메뉴 일반 hover 배경
    pub menu_hover: Rgba,
    /// 다크 모드 여부 — [`Theme::wash`] 톤 결정용
    pub is_dark: bool,
}

impl Theme {
    /// zeron `theme::wash`: 표면 위에 덧씌우는 반투명 톤. 다크는 밝은 회색, 라이트는 어두운 회색을
    /// `alpha`로 얹는다. 사이드바 톤(5%), 행 hover/선택(7%/11%) 등 "유리 위 얇은 막" 표현에 쓴다.
    pub fn wash(&self, alpha: f32) -> Rgba {
        let l = if self.is_dark { 0.92 } else { 0.10 };
        Rgba {
            r: l,
            g: l,
            b: l,
            a: alpha,
        }
    }

    /// 사이드바 열 배경 (zeron sidebar tone: wash 5%)
    pub fn sidebar_tone(&self) -> Rgba {
        self.wash(0.05)
    }

    /// 사이드바 행 선택 배경 (zeron `glass_selected_bg`: 다크 11% / 라이트 6%)
    pub fn row_selected(&self) -> Rgba {
        self.wash(if self.is_dark { 0.11 } else { 0.06 })
    }

    /// 사이드바 행/창 컨트롤 버튼 hover 배경 — 선택보다 한 단계 옅게
    pub fn row_hover(&self) -> Rgba {
        self.wash(if self.is_dark { 0.07 } else { 0.04 })
    }

    /// 좌상단 창 컨트롤 island 배경 (떠 있는 표면 — 메뉴와 같은 톤)
    pub fn island_bg(&self) -> Rgba {
        self.menu_bg
    }

    /// `color`의 불투명도를 `alpha`배로 (zeron `text_muted.opacity(0.5)` 등)
    pub fn fade(color: Rgba, alpha: f32) -> Rgba {
        Rgba {
            a: color.a * alpha,
            ..color
        }
    }

    /// `prefers-color-scheme`에 해당하는 윈도우 외관으로 라이트/다크를 결정한다.
    pub fn current(cx: &App) -> Theme {
        match cx.window_appearance() {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::dark(),
            _ => Self::light(),
        }
    }

    // ── One Light ──
    pub fn light() -> Theme {
        // 반투명 규칙: 배경/서페이스는 CC(80%), 겹치는 인터랙션 표면은 80(50%).
        // border·텍스트·작은 요소(badge/상태 점/focus ring)는 불투명.
        let one_bg = rgba(0xfafafacc); // One Light window bg
        let one_elevated = rgba(0xffffffcc); // 입력/패널
        let one_border = rgb(0xd4d4d4);
        let one_border_subtle = rgb(0xe8e8e8);
        let one_text = rgb(0x282c34);
        let one_text_subtle = rgb(0x9d9d9d); // gutter fg
        let one_hover = rgba(0xe8e8e880);
        let one_red = rgb(0xe45649);
        let one_green = rgb(0x50a14f);
        let one_yellow = rgb(0xc18401);
        let one_blue = rgb(0x4078f2);
        let one_blue_hover = rgba(0x5c86e8cc);

        Theme {
            bg_main: one_bg,
            bg_input: one_elevated,
            panel: one_elevated,
            border: one_border,
            border_subtle: one_border_subtle,
            text_main: one_text,
            text_subtle: one_text_subtle,
            text_system: one_text_subtle,
            hover: one_hover,
            accent: one_blue,
            accent_hover: one_blue_hover,
            accent_text: one_blue,
            accent_hover_bg: rgba(0x4078f280), // blue alpha 80
            focus_ring: one_blue,
            badge_bg: one_red,
            badge_text: rgb(0xffffff),
            text_inverse: rgb(0xffffff),
            error_text: one_red,
            danger_hover_bg: rgba(0xe4564980), // red alpha 80
            danger_text: one_red,
            status_connected: one_green,
            status_connecting: one_yellow,
            status_disconnected: one_red,
            overlay_scrim: rgba(0x00000080), // black alpha 80
            menu_bg: rgba(0xffffffcc),       // white CC
            menu_hover: one_hover,
            is_dark: false,
        }
    }

    // ── One Dark ──
    pub fn dark() -> Theme {
        let one_bg = rgba(0x282c34cc); // One Dark window bg
        let one_bg_input = rgba(0x2c313acc);
        let one_border = rgb(0x3e4451);
        let one_border_subtle = rgb(0x3b4048);
        let one_text = rgb(0xabb2bf);
        let one_text_subtle = rgb(0x5c6370); // gutter fg
        let one_text_system = rgb(0x7f848e);
        let one_hover = rgba(0x3b404880);
        let one_red = rgb(0xe06c75);
        let one_green = rgb(0x98c379);
        let one_yellow = rgb(0xe5c07b);
        let one_blue = rgb(0x61afef);
        let one_blue_hover = rgba(0x82aff9cc);

        Theme {
            bg_main: one_bg,
            bg_input: one_bg_input,
            panel: one_bg_input,
            border: one_border,
            border_subtle: one_border_subtle,
            text_main: one_text,
            text_subtle: one_text_subtle,
            text_system: one_text_system,
            hover: one_hover,
            accent: one_blue,
            accent_hover: one_blue_hover,
            accent_text: one_blue,
            accent_hover_bg: rgba(0x61afef80), // blue alpha 80
            focus_ring: one_blue,
            badge_bg: one_red,
            badge_text: rgb(0xffffff),
            text_inverse: rgb(0xffffff),
            error_text: one_red,
            danger_hover_bg: rgba(0xe06c7580), // red alpha 80
            danger_text: one_red,
            status_connected: one_green,
            status_connecting: one_yellow,
            status_disconnected: one_red,
            overlay_scrim: rgba(0x00000080), // black alpha 80
            menu_bg: rgba(0x21252bcc),       // One Dark 딥 패널 CC
            menu_hover: one_hover,
            is_dark: true,
        }
    }
}

/// 상태 점 색상 (connected → green, connecting/registering → yellow, else red)
pub fn status_dot_color(theme: &Theme, is_connected: bool, is_connecting: bool) -> Rgba {
    if is_connected {
        theme.status_connected
    } else if is_connecting {
        theme.status_connecting
    } else {
        theme.status_disconnected
    }
}
