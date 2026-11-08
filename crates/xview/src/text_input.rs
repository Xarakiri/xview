//! A minimal text input. GPUI ships no text field, so this is a focused
//! element that edits its text from key-down events. The symbol search uses
//! the one-line key mapping (`key_action`) directly; small fields (template
//! names, hex colors, opacity, the settings dialog's fields) use the
//! `TextInput` entity, which keeps a caret, can be multi-line (Enter inserts
//! a newline) and can show up/down steppers.

use gpui::{
    Context, EventEmitter, FocusHandle, Focusable, KeyDownEvent, Keystroke, MouseButton,
    SharedString, Window, div, prelude::*, px,
};

use crate::theme::AppTheme;
use crate::widgets::icon;

/// What a key press does to a one-line input.
#[derive(Clone, Debug, PartialEq)]
pub enum KeyAction {
    Edit(String),
    Move(isize),
    Pick,
    Dismiss,
    Paste,
    None,
}

/// Maps a keystroke to its effect on `query`.
pub fn key_action(query: &str, k: &Keystroke) -> KeyAction {
    let m = &k.modifiers;
    match k.key.as_str() {
        "escape" => return KeyAction::Dismiss,
        "enter" => return KeyAction::Pick,
        "up" => return KeyAction::Move(-1),
        "down" => return KeyAction::Move(1),
        "backspace" => {
            if m.platform || m.alt {
                return KeyAction::Edit(String::new());
            }
            let mut q = query.to_string();
            q.pop();
            return KeyAction::Edit(q);
        }
        "v" if m.platform => return KeyAction::Paste,
        _ => {}
    }
    typed_char(k).map_or(KeyAction::None, |ch| KeyAction::Edit(format!("{query}{ch}")))
}

/// The text a plain keystroke types, if any.
fn typed_char(k: &Keystroke) -> Option<&str> {
    let m = &k.modifiers;
    if m.platform || m.control || m.function {
        return None;
    }
    k.key_char
        .as_deref()
        .filter(|ch| !ch.is_empty() && !ch.chars().any(char::is_control))
}

/// Clipboard text reduced to one line of input text.
pub fn sanitize_paste(text: &str) -> String {
    text.lines()
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control())
        .collect()
}

/// Clipboard text for a multi-line field: `\r` dropped, newlines kept.
pub fn sanitize_paste_multiline(text: &str) -> String {
    text.chars()
        .filter(|&c| c == '\n' || !c.is_control())
        .collect()
}

/// A text caret.
pub fn caret(height: f32, theme: &AppTheme) -> impl IntoElement {
    div().flex_none().w(px(1.5)).h(px(height)).bg(theme.text)
}

/// Text with a caret at a byte offset (always on a char boundary).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditState {
    pub text: String,
    pub cursor: usize,
}

/// What a key did to an `EditState`.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldKey {
    Changed,
    Moved,
    Submit,
    Cancel,
    Paste,
    /// Up / down in a one-line field.
    Step(i32),
    None,
}

impl EditState {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        EditState {
            cursor: text.len(),
            text,
        }
    }

    fn prev(&self, i: usize) -> usize {
        self.text[..i].char_indices().next_back().map_or(0, |(j, _)| j)
    }

    fn next(&self, i: usize) -> usize {
        self.text[i..]
            .chars()
            .next()
            .map_or(i, |c| i + c.len_utf8())
    }

    fn line_start(&self, i: usize) -> usize {
        self.text[..i].rfind('\n').map_or(0, |j| j + 1)
    }

    fn line_end(&self, i: usize) -> usize {
        self.text[i..].find('\n').map_or(self.text.len(), |j| i + j)
    }

    /// Byte offset `col` chars into the line starting at `start` (clamped to
    /// the line end).
    fn at_col(&self, start: usize, col: usize) -> usize {
        let end = self.line_end(start);
        self.text[start..end]
            .char_indices()
            .nth(col)
            .map_or(end, |(j, _)| start + j)
    }

    /// Moves the caret one line up (`-1`) or down (`1`), keeping the column.
    fn move_line(&mut self, dir: i32) {
        let start = self.line_start(self.cursor);
        let col = self.text[start..self.cursor].chars().count();
        let target = if dir < 0 {
            if start == 0 {
                self.cursor = 0;
                return;
            }
            self.line_start(start - 1)
        } else {
            let end = self.line_end(self.cursor);
            if end == self.text.len() {
                self.cursor = end;
                return;
            }
            end + 1
        };
        self.cursor = self.at_col(target, col);
    }

    /// Inserts `s` at the caret, keeping only `accept`ed chars and at most
    /// `max_len` chars in all. Returns whether anything was inserted.
    pub fn insert(&mut self, s: &str, accept: impl Fn(char) -> bool, max_len: usize) -> bool {
        let room = max_len.saturating_sub(self.text.chars().count());
        let s: String = s.chars().filter(|&c| accept(c)).take(room).collect();
        if s.is_empty() {
            return false;
        }
        self.text.insert_str(self.cursor, &s);
        self.cursor += s.len();
        true
    }

    fn delete(&mut self, from: usize, to: usize) -> bool {
        if from == to {
            return false;
        }
        self.text.replace_range(from..to, "");
        self.cursor = from;
        true
    }

    /// Applies a keystroke. `accept` filters typed characters (a newline is
    /// always accepted in a multi-line field).
    pub fn key(
        &mut self,
        k: &Keystroke,
        multiline: bool,
        accept: impl Fn(char) -> bool,
        max_len: usize,
    ) -> FieldKey {
        let m = &k.modifiers;
        let changed = |b: bool| if b { FieldKey::Changed } else { FieldKey::None };
        let moved = |st: &mut Self, to: usize| {
            st.cursor = to;
            FieldKey::Moved
        };
        match k.key.as_str() {
            "escape" => FieldKey::Cancel,
            "enter" if multiline && !m.platform => changed(self.insert("\n", |_| true, max_len)),
            "enter" => FieldKey::Submit,
            "left" => moved(self, self.prev(self.cursor)),
            "right" => moved(self, self.next(self.cursor)),
            "home" => moved(self, self.line_start(self.cursor)),
            "end" => moved(self, self.line_end(self.cursor)),
            "up" | "down" => {
                let dir = if k.key == "up" { -1 } else { 1 };
                if multiline {
                    self.move_line(dir);
                    FieldKey::Moved
                } else {
                    FieldKey::Step(-dir)
                }
            }
            "backspace" => {
                let from = if m.platform || m.alt {
                    if multiline && self.text[..self.cursor].ends_with('\n') {
                        self.prev(self.cursor)
                    } else if multiline {
                        self.line_start(self.cursor)
                    } else {
                        0
                    }
                } else {
                    self.prev(self.cursor)
                };
                changed(self.delete(from, self.cursor))
            }
            "delete" => changed(self.delete(self.cursor, self.next(self.cursor))),
            "v" if m.platform => FieldKey::Paste,
            _ => match typed_char(k) {
                Some(ch) => {
                    let ch = ch.to_string();
                    changed(self.insert(&ch, accept, max_len))
                }
                None => FieldKey::None,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum TextInputEvent {
    Changed(String),
    Submit(String),
    Cancel,
    /// Up / down key or stepper click: `1` up, `-1` down.
    Step(i32),
}

const LINE_H: f32 = 20.;

/// A small bordered text field. `accept` filters typed and pasted
/// characters; `max_len` caps the length in characters.
pub struct TextInput {
    focus: FocusHandle,
    theme: AppTheme,
    st: EditState,
    placeholder: SharedString,
    accept: fn(char) -> bool,
    max_len: usize,
    /// Shown right after the text, e.g. "%".
    suffix: Option<SharedString>,
    /// Draws a red border (the owner validates).
    invalid: bool,
    /// Enter inserts a newline; the field is at least `min_lines` tall.
    multiline: bool,
    min_lines: usize,
    /// Up / down chevrons at the right while focused or hovered.
    stepper: bool,
    text_size: f32,
}

impl EventEmitter<TextInputEvent> for TextInput {}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TextInput {
    pub fn new(
        text: impl Into<String>,
        placeholder: impl Into<SharedString>,
        accept: fn(char) -> bool,
        max_len: usize,
        theme: AppTheme,
        cx: &mut Context<Self>,
    ) -> Self {
        TextInput {
            focus: cx.focus_handle(),
            theme,
            st: EditState::new(text),
            placeholder: placeholder.into(),
            accept,
            max_len,
            suffix: None,
            invalid: false,
            multiline: false,
            min_lines: 1,
            stepper: false,
            text_size: 14.,
        }
    }

    pub fn with_suffix(mut self, suffix: impl Into<SharedString>) -> Self {
        self.suffix = Some(suffix.into());
        self
    }

    /// A multi-line text area at least `min_lines` tall.
    pub fn multiline(mut self, min_lines: usize) -> Self {
        self.multiline = true;
        self.min_lines = min_lines.max(1);
        self
    }

    pub fn with_stepper(mut self) -> Self {
        self.stepper = true;
        self
    }

    pub fn with_text_size(mut self, size: f32) -> Self {
        self.text_size = size;
        self
    }

    pub fn text(&self) -> &str {
        &self.st.text
    }

    /// Replaces the text without emitting `Changed`; the caret goes to the
    /// end.
    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        let text = text.into();
        if text != self.st.text {
            self.st = EditState::new(text);
            cx.notify();
        }
    }

    pub fn is_invalid(&self) -> bool {
        self.invalid
    }

    pub fn set_invalid(&mut self, invalid: bool, cx: &mut Context<Self>) {
        if invalid != self.invalid {
            self.invalid = invalid;
            cx.notify();
        }
    }

    fn accepts(&self) -> impl Fn(char) -> bool + use<> {
        let (accept, multiline) = (self.accept, self.multiline);
        move |c| (multiline && c == '\n') || accept(c)
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        cx.emit(TextInputEvent::Changed(self.st.text.clone()));
        cx.notify();
    }

    fn on_key(&mut self, e: &KeyDownEvent, cx: &mut Context<Self>) {
        let accept = self.accepts();
        match self.st.key(&e.keystroke, self.multiline, &accept, self.max_len) {
            FieldKey::Changed => self.changed(cx),
            FieldKey::Moved => cx.notify(),
            FieldKey::Submit => cx.emit(TextInputEvent::Submit(self.st.text.clone())),
            FieldKey::Cancel => cx.emit(TextInputEvent::Cancel),
            FieldKey::Step(d) => cx.emit(TextInputEvent::Step(d)),
            FieldKey::Paste => {
                if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    let text = if self.multiline {
                        sanitize_paste_multiline(&text)
                    } else {
                        sanitize_paste(&text)
                    };
                    if self.st.insert(&text, &accept, self.max_len) {
                        self.changed(cx);
                    }
                }
            }
            FieldKey::None => return,
        }
        cx.stop_propagation();
    }

    fn stepper_column(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = &self.theme;
        let hover = t.hover;
        let arrow = |id: &'static str, name: &'static str, d: i32| {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .w(px(18.))
                .h(px(13.))
                .rounded(px(3.))
                .hover(move |s| s.bg(hover))
                .child(icon(name, 18., t.text))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(TextInputEvent::Step(d))))
        };
        div()
            .flex()
            .flex_col()
            .flex_none()
            .child(arrow("step-up", "chevron_up", 1))
            .child(arrow("step-down", "chevron_down", -1))
    }

    /// One line of text with the caret at byte `at` (if any).
    fn line(&self, text: &str, at: Option<usize>, focused: bool) -> impl IntoElement + use<> {
        let t = &self.theme;
        let mut row = div()
            .flex()
            .flex_row()
            .items_center()
            .flex_none()
            .h(px(LINE_H))
            .whitespace_nowrap();
        match at.filter(|_| focused) {
            Some(i) => {
                let (a, b) = text.split_at(i);
                if !a.is_empty() {
                    row = row.child(SharedString::from(a.to_string()));
                }
                row = row.child(caret(LINE_H - 2., t));
                if !b.is_empty() {
                    row = row.child(SharedString::from(b.to_string()));
                }
            }
            None => {
                if !text.is_empty() {
                    row = row.child(SharedString::from(text.to_string()));
                }
            }
        }
        row
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme.clone();
        let focused = self.focus.is_focused(window);
        let empty = self.st.text.is_empty();
        let mut placeholder = (empty && self.suffix.is_none()).then(|| {
            div()
                .pl(px(2.))
                .text_color(t.muted)
                .whitespace_nowrap()
                .child(self.placeholder.clone())
        });
        let border = if self.invalid {
            t.badge
        } else if focused {
            t.accent
        } else {
            t.border
        };
        // The focus ring is 2 px; the padding shrinks so text stays put.
        let pad = if focused || self.invalid { 1. } else { 0. };
        let mut field = div()
            .id("text-input")
            .group("text-input")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| this.on_key(e, cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    if !this.focus.is_focused(window) {
                        this.st.cursor = this.st.text.len();
                        window.focus(&this.focus, cx);
                    }
                }),
            )
            .flex()
            .rounded(px(6.))
            .when(focused || self.invalid, |d| d.border_2())
            .when(!focused && !self.invalid, |d| d.border_1())
            .border_color(border)
            .bg(t.bg)
            .text_color(t.text)
            .text_size(px(self.text_size))
            .overflow_hidden();
        if self.multiline {
            let lines: Vec<&str> = self.st.text.split('\n').collect();
            let mut start = 0;
            let mut col = div().flex().flex_col().flex_1().min_w_0();
            for (i, l) in lines.into_iter().enumerate() {
                let end = start + l.len();
                let cur = self.st.cursor;
                let at = (cur >= start && cur <= end).then(|| cur - start);
                if i == 0 && empty {
                    col = col.child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .h(px(LINE_H))
                            .when(focused, |d| d.child(caret(LINE_H - 2., &t)))
                            .children(placeholder.take()),
                    );
                } else {
                    col = col.child(self.line(l, at, focused));
                }
                start = end + 1;
            }
            let min_h = self.min_lines as f32 * LINE_H + 16. - 2. * pad;
            field = field
                .flex_col()
                .min_h(px(min_h + 2. * pad))
                .px(px(12. - pad))
                .py(px(8. - pad))
                .child(col);
        } else {
            field = field
                .flex_row()
                .items_center()
                .h(px(34.))
                .pl(px(10. - pad))
                .pr(px(8. - pad))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .child(self.line(&self.st.text, Some(self.st.cursor), focused))
                        .when_some(self.suffix.clone(), |d, s| d.child(s))
                        .children(placeholder),
                )
                .when(self.stepper, |d| {
                    let steps = self.stepper_column(cx);
                    // Laid over the text's right end, only while focused or
                    // hovered, so narrow fields keep their full width.
                    d.relative().child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .right(px(3. - pad))
                            .flex()
                            .items_center()
                            .bg(t.bg)
                            .when(!focused, |d| {
                                d.invisible().group_hover("text-input", |s| s.visible())
                            })
                            .child(steps),
                    )
                });
        }
        field
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::Modifiers;

    fn ks(key: &str, ch: Option<&str>, platform: bool) -> Keystroke {
        Keystroke {
            modifiers: Modifiers {
                platform,
                ..Modifiers::default()
            },
            key: key.into(),
            key_char: ch.map(Into::into),
        }
    }

    fn any(_: char) -> bool {
        true
    }

    #[test]
    fn multiline_enter_inserts_newline_and_cmd_enter_submits() {
        let mut st = EditState::new("ab");
        assert_eq!(st.key(&ks("enter", None, false), true, any, 100), FieldKey::Changed);
        assert_eq!(st.text, "ab\n");
        assert_eq!(st.key(&ks("enter", None, true), true, any, 100), FieldKey::Submit);
        let mut one = EditState::new("ab");
        assert_eq!(one.key(&ks("enter", None, false), false, any, 100), FieldKey::Submit);
        assert_eq!(one.text, "ab");
    }

    #[test]
    fn caret_moves_and_edits_in_place() {
        let mut st = EditState::new("héllo");
        st.key(&ks("left", None, false), false, any, 100);
        st.key(&ks("left", None, false), false, any, 100);
        st.key(&ks("x", Some("x"), false), false, any, 100);
        assert_eq!(st.text, "hélxlo");
        st.key(&ks("backspace", None, false), false, any, 100);
        assert_eq!(st.text, "héllo");
        // Back over a two-byte char.
        st.cursor = 3;
        st.key(&ks("backspace", None, false), false, any, 100);
        assert_eq!((st.text.as_str(), st.cursor), ("hllo", 1));
        st.key(&ks("delete", None, false), false, any, 100);
        assert_eq!(st.text, "hlo");
        st.key(&ks("right", None, false), false, any, 100);
        st.key(&ks("right", None, false), false, any, 100);
        st.key(&ks("right", None, false), false, any, 100);
        assert_eq!(st.cursor, 3);
    }

    #[test]
    fn up_down_keep_the_column_or_step() {
        let mut st = EditState::new("abcd\nx\nwxyz");
        st.cursor = 3; // after "abc"
        st.key(&ks("down", None, false), true, any, 100);
        assert_eq!(st.cursor, 6); // end of "x"
        st.key(&ks("down", None, false), true, any, 100);
        assert_eq!(st.cursor, 8); // "w|xyz": column 1
        st.key(&ks("up", None, false), true, any, 100);
        st.key(&ks("up", None, false), true, any, 100);
        assert_eq!(st.cursor, 1);
        st.key(&ks("up", None, false), true, any, 100);
        assert_eq!(st.cursor, 0);
        let mut one = EditState::new("1.5");
        assert_eq!(one.key(&ks("up", None, false), false, any, 100), FieldKey::Step(1));
        assert_eq!(one.key(&ks("down", None, false), false, any, 100), FieldKey::Step(-1));
    }

    #[test]
    fn insert_filters_and_caps() {
        let mut st = EditState::new("12");
        st.cursor = 1;
        assert!(st.insert("a3b4", |c| c.is_ascii_digit(), 3));
        assert_eq!((st.text.as_str(), st.cursor), ("132", 2));
        assert!(!st.insert("5", |c| c.is_ascii_digit(), 3));
        assert_eq!(sanitize_paste_multiline("a\r\nb\tc"), "a\nbc");
    }

    #[test]
    fn cmd_backspace_deletes_to_line_start() {
        let mut st = EditState::new("ab\ncd");
        st.key(&ks("backspace", None, true), true, any, 100);
        assert_eq!(st.text, "ab\n");
        st.key(&ks("backspace", None, true), true, any, 100);
        assert_eq!(st.text, "ab");
        let mut one = EditState::new("abc");
        one.cursor = 2;
        one.key(&ks("backspace", None, true), false, any, 100);
        assert_eq!(one.text, "c");
    }
}
