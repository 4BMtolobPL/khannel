//! GPUI Actions (MIGRATION.md `crates/kirc-app/action.rs`).
//!
//! Phase 2에서는 종료 흐름용 `Quit` 액션만 정의한다.
//! 컨텍스트 메뉴/모달 액션은 Phase 3에서 확장.

use gpui::actions;

actions!(kirc, [Quit]);
