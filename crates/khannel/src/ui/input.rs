//! TextInput: IME(한글 등) 입력을 지원하는 단일 줄 텍스트 입력 엔티티.
//!
//! gpui `examples/input.rs`의 정석 구현을 포팅했다:
//! - [`EntityInputHandler`] 구현 + `window.handle_input` 등록 → 플랫폼 IME(X11 XIM /
//!   Wayland text-input, macOS NSTextInputClient)가 이 엔티티로 텍스트를 주입한다.
//! - 조합 중(marked text)일 때의 밑줄 표시, 커서/선택 영역 렌더링 포함.
//! - Enter 제출은 조합 중이 아닐 때만 발화한다(`TextInputEvent::Submit`).
//!
//! Phase 2의 `handle_composer_key`(키 네이밍 기반 간이 composer)를 이 컴포넌트로 교체하는
//! 것이 docs/MIGRATION.md Phase 3의 목표다.

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, ContentMask, Context, CursorStyle, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    GlobalElementId, InspectorElementId, InteractiveElement, KeyDownEvent, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, ParentElement, Pixels, Point, Render,
    ShapedLine, SharedString, Style, TextRun, UTF16Selection, UnderlineStyle, Window, div, fill,
    prelude::*, px, rgba,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::action::{
    Backspace, Copy, Cut, Delete, DeleteWordLeft, DeleteWordRight, End, Home, Left, Paste, Right,
    SelectAll, SelectLeft, SelectRight, SelectWordLeft, SelectWordRight, ShowCharacterPalette,
    WordLeft, WordRight,
};
use crate::theme::Theme;

// ==================================== 이벤트 ====================================

#[derive(Debug)]
pub enum TextInputEvent {
    /// Enter로 제출 (IME 조합 중이 아닐 때만). 제출 시점의 텍스트를 담는다.
    Submit(SharedString),
    /// 내용 변경 — 구독자는 [`TextInput::text`]로 현재 내용을 읽는다.
    Changed,
}

impl EventEmitter<TextInputEvent> for TextInput {}

// ==================================== 엔티티 ====================================

pub struct TextInput {
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    /// 가로 스크롤 오프셋 — 내용이 입력창보다 길 때 커서가 보이도록 왼쪽으로 민 양.
    /// prepaint에서 커서 위치로 재계산하고 paint에서 저장한다.
    scroll_x: Pixels,
    is_selecting: bool,
    /// 잠긴 채널 등에서 입력 비활성화 (Svelte `disabled`)
    enabled: bool,
}

impl TextInput {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: "".into(),
            placeholder: "".into(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            scroll_x: px(0.),
            is_selecting: false,
            enabled: true,
        }
    }

    pub fn with_placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// 초기 텍스트 (모달 기본값 등). 커서는 끝에 둔다.
    pub fn with_text(mut self, text: impl Into<SharedString>) -> Self {
        self.content = text.into();
        self.selected_range = self.content.len()..self.content.len();
        self
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus_handle.is_focused(window)
    }

    /// 활성화/비활성화. 변경 시에만 notify한다(렌더 중 호출해도 루프 없음).
    pub fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.enabled != enabled {
            self.enabled = enabled;
            cx.notify();
        }
    }

    /// 플레이스홀더 변경. 변경 시에만 notify한다.
    pub fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let placeholder = placeholder.into();
        if self.placeholder != placeholder {
            self.placeholder = placeholder;
            cx.notify();
        }
    }

    /// 내용을 통째로 교체하고 커서를 끝으로 이동한다.
    pub fn set_text(
        &mut self,
        text: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.content = text.into();
        self.selected_range = self.content.len()..self.content.len();
        self.selection_reversed = false;
        self.marked_range = None;
        window.notify_text_input_changed(&self.focus_handle);
        cx.emit(TextInputEvent::Changed);
        cx.notify();
    }

    /// 내용을 비운다.
    pub fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_text("", window, cx);
    }

    // ---- 커서/선택 이동 (examples/input.rs) ----

    fn left(&mut self, _: &Left, window: &mut Window, cx: &mut Context<Self>) {
        if !self.enabled {
            return;
        }
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), window, cx);
        } else {
            self.move_to(self.selected_range.start, window, cx)
        }
    }

    fn right(&mut self, _: &Right, window: &mut Window, cx: &mut Context<Self>) {
        if !self.enabled {
            return;
        }
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), window, cx);
        } else {
            self.move_to(self.selected_range.end, window, cx)
        }
    }

    fn select_left(&mut self, _: &SelectLeft, window: &mut Window, cx: &mut Context<Self>) {
        if !self.enabled {
            return;
        }
        self.select_to(self.previous_boundary(self.cursor_offset()), window, cx);
    }

    fn select_right(&mut self, _: &SelectRight, window: &mut Window, cx: &mut Context<Self>) {
        if !self.enabled {
            return;
        }
        self.select_to(self.next_boundary(self.cursor_offset()), window, cx);
    }

    fn select_all(&mut self, _: &SelectAll, window: &mut Window, cx: &mut Context<Self>) {
        if !self.enabled {
            return;
        }
        self.move_to(0, window, cx);
        self.select_to(self.content.len(), window, cx)
    }

    fn home(&mut self, _: &Home, window: &mut Window, cx: &mut Context<Self>) {
        if !self.enabled {
            return;
        }
        self.move_to(0, window, cx);
    }

    fn end(&mut self, _: &End, window: &mut Window, cx: &mut Context<Self>) {
        if !self.enabled {
            return;
        }
        self.move_to(self.content.len(), window, cx);
    }

    // ---- 단어 단위 이동/선택/삭제 ----

    fn word_left(&mut self, _: &WordLeft, window: &mut Window, cx: &mut Context<Self>) {
        if !self.enabled {
            return;
        }
        let target = previous_word_boundary(&self.content, self.cursor_offset());
        self.move_to(target, window, cx);
    }

    fn word_right(&mut self, _: &WordRight, window: &mut Window, cx: &mut Context<Self>) {
        if !self.enabled {
            return;
        }
        let target = next_word_boundary(&self.content, self.cursor_offset());
        self.move_to(target, window, cx);
    }

    fn select_word_left(
        &mut self,
        _: &SelectWordLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.enabled {
            return;
        }
        let target = previous_word_boundary(&self.content, self.cursor_offset());
        self.select_to(target, window, cx);
    }

    fn select_word_right(
        &mut self,
        _: &SelectWordRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.enabled {
            return;
        }
        let target = next_word_boundary(&self.content, self.cursor_offset());
        self.select_to(target, window, cx);
    }

    /// 선택이 있으면 선택을, 없으면 커서 앞 단어를 지운다.
    fn delete_word_left(
        &mut self,
        _: &DeleteWordLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.enabled {
            return;
        }
        if self.selected_range.is_empty() {
            let target = previous_word_boundary(&self.content, self.cursor_offset());
            if target == self.cursor_offset() {
                return;
            }
            self.select_to(target, window, cx);
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    /// 선택이 있으면 선택을, 없으면 커서 뒤 단어를 지운다.
    fn delete_word_right(
        &mut self,
        _: &DeleteWordRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.enabled {
            return;
        }
        if self.selected_range.is_empty() {
            let target = next_word_boundary(&self.content, self.cursor_offset());
            if target == self.cursor_offset() {
                return;
            }
            self.select_to(target, window, cx);
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if !self.enabled {
            return;
        }
        if self.selected_range.is_empty() {
            let prev = self.previous_boundary(self.cursor_offset());
            if self.cursor_offset() == prev {
                window.play_system_bell();
                return;
            }
            self.select_to(prev, window, cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if !self.enabled {
            return;
        }
        if self.selected_range.is_empty() {
            let next = self.next_boundary(self.cursor_offset());
            if self.cursor_offset() == next {
                window.play_system_bell();
                return;
            }
            self.select_to(next, window, cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if !self.enabled {
            return;
        }
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            // 한 줄 입력이므로 줄바꿈(CRLF/CR/LF)은 공백으로. `\r`이 남으면 irc 코덱이 거기서
            // 메시지를 잘라 서버엔 첫 줄만 가고 로컬 에코는 전체가 보인다.
            let text = text.replace("\r\n", " ").replace(['\r', '\n'], " ");
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    /// macOS 문자 뷰어(이모지/기호) — ctrl-cmd-space
    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        if self.enabled {
            window.show_character_palette();
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.enabled || self.selected_range.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(
            self.content[self.selected_range.clone()].to_string(),
        ));
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.enabled || self.selected_range.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(
            self.content[self.selected_range.clone()].to_string(),
        ));
        self.replace_text_in_range(None, "", window, cx)
    }

    // ---- 마우스 ----

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.enabled {
            return;
        }
        self.is_selecting = true;
        window.request_virtual_keyboard();

        if event.modifiers.shift {
            self.select_to(self.index_for_mouse_position(event.position), window, cx);
        } else {
            self.move_to(self.index_for_mouse_position(event.position), window, cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.enabled {
            return;
        }
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), window, cx);
        }
    }

    // ---- 내부 유틸 (examples/input.rs) ----

    fn move_to(&mut self, offset: usize, window: &Window, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        window.notify_text_selection_changed(&self.focus_handle);
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }

        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        line.closest_index_for_x(position.x - bounds.left() + self.scroll_x)
    }

    fn select_to(&mut self, offset: usize, window: &Window, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        window.notify_text_selection_changed(&self.focus_handle);
        cx.notify()
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;

        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }

        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;

        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }

        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }

    fn notify_changed(&mut self, cx: &mut Context<Self>) {
        cx.emit(TextInputEvent::Changed);
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn set_selected_text_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected_range = self.range_from_utf16(&range_utf16);
        self.selection_reversed = false;
        window.notify_text_selection_changed(&self.focus_handle);
        cx.notify();
    }

    fn text_length_utf16(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.content.encode_utf16().count())
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.marked_range = None;
        window.notify_text_input_changed(&self.focus_handle);
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 플랫폼 IME가 직접 호출하는 경로 — 키 액션과 달리 여기서 가드해야
        // 잠금(disabled) 상태에서도 입력이 주입되지 않는다.
        if !self.enabled {
            return;
        }
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.selection_reversed = false;
        self.marked_range.take();
        window.notify_text_input_changed(&self.focus_handle);
        self.notify_changed(cx);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.enabled {
            return;
        }
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        } else {
            self.marked_range = None;
        }
        let insertion_start_utf16 = self.offset_to_utf16(range.start);
        let inserted_length_utf16 = new_text.encode_utf16().count();
        self.selected_range = new_selected_range_utf16
            .map(|selected| {
                self.range_from_utf16(
                    &(insertion_start_utf16 + selected.start.min(inserted_length_utf16)
                        ..insertion_start_utf16 + selected.end.min(inserted_length_utf16)),
                )
            })
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        self.selection_reversed = false;

        window.notify_text_input_changed(&self.focus_handle);
        self.notify_changed(cx);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let last_layout = self.last_layout.as_ref()?;
        if last_layout.text != self.content {
            return None;
        }
        let range = self.range_from_utf16(&range_utf16);
        let left = bounds.left() - self.scroll_x;
        Some(Bounds::from_corners(
            gpui::point(left + last_layout.x_for_index(range.start), bounds.top()),
            gpui::point(left + last_layout.x_for_index(range.end), bounds.bottom()),
        ))
    }

    fn selection_bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Bounds<Pixels>> {
        self.bounds_for_range(range_utf16, bounds, window, cx)
            .into_iter()
            .collect()
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let line_point = self.last_bounds?.localize(&point)?;
        let last_layout = self.last_layout.as_ref()?;

        if last_layout.text != self.content {
            return None;
        }
        let utf8_index = last_layout.closest_index_for_x(line_point.x + self.scroll_x);
        Some(self.offset_to_utf16(utf8_index))
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ==================================== 엘리먼트 ====================================

/// TextInput의 실제 렌더(텍스트/커서/선택 영역)를 담당하는 커스텀 엘리먼트.
/// paint 시점에 `window.handle_input`으로 IME 핸들러를 등록하는 것이 핵심.
struct TextInputElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
    scroll_x: Pixels,
}

/// 커서 너비 (prepaint의 커서 quad와 동일).
const CURSOR_WIDTH: Pixels = px(2.);

/// 커서가 입력창 안에 보이도록 가로 스크롤 오프셋을 조정한다.
///
/// 커서가 오른쪽 끝을 넘으면 오른쪽으로, 왼쪽 끝을 넘으면 왼쪽으로 필요한 만큼만 민다.
/// 내용이 입력창에 다 들어오면 0, 내용 끝 너머로는 스크롤하지 않는다.
fn adjust_scroll_x(
    scroll_x: Pixels,
    cursor_x: Pixels,
    line_width: Pixels,
    view_width: Pixels,
) -> Pixels {
    let mut scroll_x = scroll_x;
    if cursor_x + CURSOR_WIDTH > scroll_x + view_width {
        scroll_x = cursor_x + CURSOR_WIDTH - view_width;
    }
    if cursor_x < scroll_x {
        scroll_x = cursor_x;
    }
    let max_scroll = line_width + CURSOR_WIDTH - view_width;
    if scroll_x > max_scroll {
        scroll_x = max_scroll;
    }
    if scroll_x < px(0.) {
        scroll_x = px(0.);
    }
    scroll_x
}

impl IntoElement for TextInputElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextInputElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = gpui::relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = input.content.clone();
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let enabled = input.enabled;
        let style = window.text_style();
        let theme = Theme::current(cx);

        let (display_text, text_color) = if content.is_empty() {
            // 플레이스홀더: Svelte 입력창의 흐린 텍스트 톤
            (input.placeholder.clone(), theme.text_subtle.into())
        } else if enabled {
            (content, style.color)
        } else {
            // 비활성화(Svelte `disabled`): 본문도 흐리게
            (content, theme.text_subtle.into())
        };

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked_range) = input.marked_range.as_ref() {
            vec![
                TextRun {
                    len: marked_range.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked_range.end - marked_range.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: display_text.len() - marked_range.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display_text, font_size, &runs, None);

        let cursor_pos = line.x_for_index(cursor);
        // 플레이스홀더 표시 중(내용 없음)에는 스크롤하지 않는다.
        let scroll_x = if input.content.is_empty() {
            px(0.)
        } else {
            adjust_scroll_x(input.scroll_x, cursor_pos, line.width(), bounds.size.width)
        };
        let left = bounds.left() - scroll_x;
        let (selection, cursor) = if selected_range.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(
                        gpui::point(left + cursor_pos, bounds.top()),
                        gpui::size(CURSOR_WIDTH, bounds.bottom() - bounds.top()),
                    ),
                    gpui::blue(),
                )),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        gpui::point(left + line.x_for_index(selected_range.start), bounds.top()),
                        gpui::point(left + line.x_for_index(selected_range.end), bounds.bottom()),
                    ),
                    rgba(0x3311ff30),
                )),
                None,
            )
        };
        PrepaintState {
            line: Some(line),
            cursor,
            selection,
            scroll_x,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        let line = prepaint.line.take().unwrap();
        let scroll_x = prepaint.scroll_x;
        let show_cursor = focus_handle.is_focused(window) && self.input.read(cx).is_enabled();

        // 입력창 경계 밖(Send 버튼 등)으로 텍스트/선택 영역이 넘쳐 그려지지 않게 clip한다.
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            if let Some(selection) = prepaint.selection.take() {
                window.paint_quad(selection)
            }
            line.paint(
                gpui::point(bounds.left() - scroll_x, bounds.top()),
                window.line_height(),
                gpui::TextAlign::Left,
                None,
                window,
                cx,
            )
            .unwrap();

            if show_cursor && let Some(cursor) = prepaint.cursor.take() {
                window.paint_quad(cursor);
            }
        });

        self.input.update(cx, |input, _cx| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
            input.scroll_x = scroll_x;
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let enabled = self.enabled;

        div()
            .flex()
            .key_context("TextInput")
            .track_focus(&self.focus_handle)
            .when(enabled, |d| d.cursor(CursorStyle::IBeam))
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::delete_word_left))
            .on_action(cx.listener(Self::delete_word_right))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::show_character_palette))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                // Enter로 제출 — IME 조합(한글) 중에는 제출하지 않는다.
                if event.keystroke.key == "enter" && this.enabled && this.marked_range.is_none() {
                    cx.emit(TextInputEvent::Submit(this.content.clone()));
                    cx.stop_propagation();
                }
            }))
            // 비활성화(Svelte `disabled`): 입력 요소를 흐리게 표시
            .when(!enabled, |d| d.opacity(0.5))
            .child(TextInputElement { input: cx.entity() })
    }
}

/// `offset` 앞쪽의 단어 시작 위치. 공백을 건너뛴 뒤 그 단어의 처음으로 간다.
///
/// 단어 경계는 유니코드 단어 분할(UAX #29)을 따른다 — 한글은 붙어 있는 음절이 한 단어,
/// `/` `,` 같은 구두점은 그 자체가 하나의 정지 지점이다. 글자 사이의 `.`은 단어를 끊지 않아
/// `irc.libera.chat` 같은 주소는 한 단어로 다룬다.
fn previous_word_boundary(text: &str, offset: usize) -> usize {
    text.split_word_bound_indices()
        .rev()
        .find(|(start, segment)| *start < offset && !segment.trim().is_empty())
        .map(|(start, _)| start)
        .unwrap_or(0)
}

/// `offset` 뒤쪽의 단어 끝 위치. 공백을 건너뛴 뒤 그 단어의 끝으로 간다.
fn next_word_boundary(text: &str, offset: usize) -> usize {
    text.split_word_bound_indices()
        .find(|(start, segment)| start + segment.len() > offset && !segment.trim().is_empty())
        .map(|(start, segment)| start + segment.len())
        .unwrap_or(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW: Pixels = px(100.);

    #[test]
    fn word_boundaries_skip_spaces_and_stop_at_words() {
        let text = "hello  world foo";
        // 단어 중간/끝에서 → 그 단어의 처음
        assert_eq!(previous_word_boundary(text, 9), 7);
        assert_eq!(previous_word_boundary(text, 12), 7);
        // 단어 처음에서 → 앞 공백을 건너뛰어 앞 단어의 처음
        assert_eq!(previous_word_boundary(text, 7), 0);
        assert_eq!(previous_word_boundary(text, 0), 0);

        assert_eq!(next_word_boundary(text, 0), 5);
        // 공백 위에서 → 다음 단어의 끝
        assert_eq!(next_word_boundary(text, 5), 12);
        assert_eq!(next_word_boundary(text, 16), 16);
    }

    #[test]
    fn word_boundaries_handle_korean_and_punctuation() {
        let text = "안녕하세요 반가워요";
        let second = "안녕하세요 ".len();
        assert_eq!(previous_word_boundary(text, text.len()), second);
        assert_eq!(next_word_boundary(text, 0), "안녕하세요".len());

        // 단어를 끊는 구두점은 그 자체로 정지 지점
        let text = "foo/bar";
        assert_eq!(previous_word_boundary(text, 7), 4);
        assert_eq!(previous_word_boundary(text, 4), 3);
        assert_eq!(next_word_boundary(text, 0), 3);

        // 글자 사이의 `.`은 단어를 끊지 않는다 (UAX #29) — 서버 주소는 한 단어
        let text = "join irc.libera.chat";
        assert_eq!(previous_word_boundary(text, text.len()), 5);
    }

    #[test]
    fn short_text_never_scrolls() {
        assert_eq!(adjust_scroll_x(px(0.), px(50.), px(60.), VIEW), px(0.));
        // 이전 오프셋이 남아 있어도 내용이 다 들어오면 0으로 돌아온다 (예: 텍스트 삭제 후)
        assert_eq!(adjust_scroll_x(px(40.), px(50.), px(60.), VIEW), px(0.));
    }

    #[test]
    fn cursor_past_right_edge_scrolls_just_enough() {
        // 커서(200) + 커서 너비(2)가 오른쪽 끝에 붙도록
        assert_eq!(adjust_scroll_x(px(0.), px(200.), px(200.), VIEW), px(102.));
    }

    #[test]
    fn cursor_past_left_edge_scrolls_back() {
        assert_eq!(adjust_scroll_x(px(102.), px(30.), px(200.), VIEW), px(30.));
        // Home: 맨 앞으로
        assert_eq!(adjust_scroll_x(px(102.), px(0.), px(200.), VIEW), px(0.));
    }

    #[test]
    fn cursor_inside_view_keeps_offset() {
        assert_eq!(adjust_scroll_x(px(50.), px(80.), px(200.), VIEW), px(50.));
    }
}
