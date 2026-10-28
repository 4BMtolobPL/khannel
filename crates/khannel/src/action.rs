//! GPUI Actions (MIGRATION.md 목표 구조의 `action.rs`).
//!
//! Phase 2에서는 종료 흐름용 `Quit` 액션만 정의.
//! Phase 3에서 TextInput(IME 지원 입력창)의 표준 편집 액션을 추가한다.

use gpui::actions;

actions!(khannel, [Quit]);

// 모달/컨텍스트 메뉴의 Esc 닫기 액션 (Phase 4 패리티 수정 — Svelte `<dialog>`의 Esc 대체)
actions!(khannel, [Dismiss]);

// TextInput 표준 편집 액션 (gpui examples/input.rs 포팅)
actions!(
    khannel,
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
        // 단어 단위 이동/선택/삭제 (Linux ctrl / macOS option 조합)
        WordLeft,
        WordRight,
        SelectWordLeft,
        SelectWordRight,
        DeleteWordLeft,
        DeleteWordRight,
        ShowCharacterPalette,
        Paste,
        Cut,
        Copy,
    ]
);
