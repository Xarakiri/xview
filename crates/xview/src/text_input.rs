//! A minimal one-line text input. GPUI ships no text field, so this is a
//! focused element that edits its text from key-down events (typing,
//! backspace, cmd-backspace, cmd-v). The symbol search uses the key mapping
//! directly; small fields (template names, hex colors, opacity) use the
//! `TextInput` entity.

use gpui::{
    Context, EventEmitter, FocusHandle, Focusable, KeyDownEvent, Keystroke, MouseButton,
    SharedString, Window, div, prelude::*, px,
};

use crate::theme::AppTheme;

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
    if m.platform || m.control || m.function {
        return KeyAction::None;
    }
    match k.key_char.as_deref() {
        Some(ch) if !ch.is_empty() && !ch.chars().any(char::is_control) => {
            KeyAction::Edit(format!("{query}{ch}"))
        }
        _ => KeyAction::None,
    }
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

/// A text caret.
pub fn caret(height: f32, theme: &AppTheme) -> impl IntoElement {
    div().flex_none().w(px(1.5)).h(px(height)).bg(theme.text)
}

#[derive(Clone, Debug, PartialEq)]
pub enum TextInputEvent {
    Changed(String),
    Submit(String),
    Cancel,
}

/// A small bordered one-line field. `accept` filters typed and pasted
/// characters; `max_len` caps the length in characters.
pub struct TextInput {
    focus: FocusHandle,
    theme: AppTheme,
    text: String,
    placeholder: SharedString,
    accept: fn(char) -> bool,
    max_len: usize,
    /// Shown right after the text, e.g. "%".
    suffix: Option<SharedString>,
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
            text: text.into(),
            placeholder: placeholder.into(),
            accept,
            max_len,
            suffix: None,
        }
    }

    pub fn with_suffix(mut self, suffix: impl Into<SharedString>) -> Self {
        self.suffix = Some(suffix.into());
        self
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Replaces the text without emitting `Changed`.
    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        let text = text.into();
        if text != self.text {
            self.text = text;
            cx.notify();
        }
    }

    fn filtered(&self, s: &str) -> String {
        s.chars()
            .filter(|&c| (self.accept)(c))
            .take(self.max_len)
            .collect()
    }

    fn edit(&mut self, text: String, cx: &mut Context<Self>) {
        let text = self.filtered(&text);
        if text != self.text {
            self.text = text.clone();
            cx.emit(TextInputEvent::Changed(text));
            cx.notify();
        }
    }

    fn on_key(&mut self, e: &KeyDownEvent, cx: &mut Context<Self>) {
        match key_action(&self.text, &e.keystroke) {
            KeyAction::Edit(t) => self.edit(t, cx),
            KeyAction::Pick => cx.emit(TextInputEvent::Submit(self.text.clone())),
            KeyAction::Dismiss => cx.emit(TextInputEvent::Cancel),
            KeyAction::Paste => {
                if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    let t = format!("{}{}", self.text, sanitize_paste(&text));
                    self.edit(t, cx);
                }
            }
            KeyAction::Move(_) | KeyAction::None => return,
        }
        cx.stop_propagation();
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let focused = self.focus.is_focused(window);
        let text: SharedString = self.text.clone().into();
        div()
            .id("text-input")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| this.on_key(e, cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    window.focus(&this.focus, cx);
                }),
            )
            .flex()
            .flex_row()
            .items_center()
            .h(px(34.))
            .px(px(8.))
            .rounded(px(6.))
            .border_1()
            .border_color(if focused { t.accent } else { t.border })
            .bg(t.bg)
            .text_color(t.text)
            .text_size(px(14.))
            .overflow_hidden()
            .when(!self.text.is_empty(), |d| d.child(text))
            .when(focused, |d| d.child(caret(18., t)))
            .when_some(self.suffix.clone(), |d, s| d.child(s))
            .when(self.text.is_empty() && self.suffix.is_none(), |d| {
                d.child(
                    div()
                        .pl(px(2.))
                        .text_color(t.muted)
                        .child(self.placeholder.clone()),
                )
            })
    }
}
