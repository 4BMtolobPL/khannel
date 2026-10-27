//! GPUI Actions (MIGRATION.md `crates/kirc-app/action.rs`).
//!
//! Phase 2에서는 종료 흐름용 `Quit` 액션만 정의.
//! Phase 3에서 TextInput(IME 지원 입력창)의 표준 편집 액션을 추가한다.

use gpui::actions;

actions!(kirc, [Quit]);

// 모달/컨텍스트 메뉴의 Esc 닫기 액션 (Phase 4 패리티 수정 — Svelte `<dialog>`의 Esc 대체)
actions!(kirc, [Dismiss]);

// TextInput 표준 편집 액션 (gpui examples/input.rs 포팅)
actions!(
    kirc,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        ShowCharacterPalette,
        Paste,
        Cut,
        Copy,
    ]
);
