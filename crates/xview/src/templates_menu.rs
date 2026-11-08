//! The drawing templates menu, shared by the floating toolbar and the
//! settings dialog: "Save Drawing Template As…" (with an inline name field),
//! "Apply Default Drawing Template" and the tool's named templates, each with
//! a delete button on hover.

use gpui::{Context, Div, Entity, SharedString, Window, div, prelude::*, px};
use xv_drawings::{Drawing, Style, Tool};
use xv_store::NamedTemplate;

use crate::text_input::TextInput;
use crate::theme::AppTheme;
use crate::widgets::{DISABLED_OPACITY, menu_box, menu_row};

pub const TEMPLATE_NAME_MAX: usize = 40;

/// `d`'s style replaced by `template`, keeping the drawing's text.
pub fn apply_template(d: &mut Drawing, template: &Style) {
    let content = std::mem::take(&mut d.style.text.content);
    d.style = template.clone();
    d.style.text.content = content;
}

/// The style saved as a template: the look, not the words.
pub fn template_style(style: &Style) -> Style {
    let mut style = style.clone();
    style.text.content.clear();
    style
}

/// What the menu's rows do; implemented by its owner.
pub trait TemplateHost: Sized + 'static {
    /// "Save Drawing Template As…": show the name field.
    fn open_template_name(&mut self, window: &mut Window, cx: &mut Context<Self>);
    /// Save under `name` (from the name field).
    fn save_template(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>);
    /// Hide the name field.
    fn cancel_template_name(&mut self, window: &mut Window, cx: &mut Context<Self>);
    /// Apply `style` (the default or a named template).
    fn apply_template_style(&mut self, style: Style, window: &mut Window, cx: &mut Context<Self>);
    /// Delete the named template `name`.
    fn delete_template(&mut self, name: String, cx: &mut Context<Self>);
}

/// A new name field for "Save As…".
pub fn name_input(theme: &AppTheme, cx: &mut gpui::App) -> Entity<TextInput> {
    let theme = theme.clone();
    cx.new(|cx| {
        TextInput::new(
            "",
            "Template name",
            |c| !c.is_control(),
            TEMPLATE_NAME_MAX,
            theme,
            cx,
        )
    })
}

/// The menu for `tool`; `name_input` is the open "Save As…" field.
pub fn templates_menu<T: TemplateHost>(
    tool: Tool,
    named: &[NamedTemplate],
    name_input: Option<&Entity<TextInput>>,
    t: &AppTheme,
    cx: &mut Context<T>,
) -> Div {
    let mut menu = menu_box(t).min_w(px(260.)).child(
        menu_row(
            "tpl-save",
            None,
            "Save Drawing Template As…",
            false,
            true,
            t,
        )
        .text_size(px(15.))
        .on_click(cx.listener(|this, _, window, cx| this.open_template_name(window, cx))),
    );
    if let Some(input) = name_input {
        let save_input = input.clone();
        let can_save = !input.read(cx).text().trim().is_empty();
        menu = menu.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.))
                .px(px(12.))
                .py(px(4.))
                .child(div().flex_1().child(input.clone()))
                .child(
                    small_button("tpl-name-save", "Save", true, t)
                        .when(!can_save, |b| b.opacity(DISABLED_OPACITY))
                        .when(can_save, |b| {
                            b.on_click(cx.listener(move |this, _, window, cx| {
                                let name = save_input.read(cx).text().to_string();
                                this.save_template(name, window, cx);
                            }))
                        }),
                )
                .child(
                    small_button("tpl-name-cancel", "Cancel", false, t).on_click(
                        cx.listener(|this, _, window, cx| this.cancel_template_name(window, cx)),
                    ),
                ),
        );
    }
    menu = menu.child(
        menu_row(
            "tpl-default",
            None,
            "Apply Default Drawing Template",
            false,
            true,
            t,
        )
        .text_size(px(15.))
        .on_click(cx.listener(move |this, _, window, cx| {
            this.apply_template_style(tool.default_style(), window, cx)
        })),
    );
    if !named.is_empty() {
        menu = menu.child(div().h(px(1.)).mx(px(8.)).my(px(4.)).bg(t.border));
    }
    for (i, nt) in named.iter().enumerate() {
        let group = SharedString::from(format!("tpl-row-{i}"));
        let name = nt.name.clone();
        let style = nt.style.clone();
        let hover = t.hover;
        menu = menu.child(
            menu_row(group.clone(), None, nt.name.clone(), false, true, t)
                .group(group.clone())
                .text_size(px(15.))
                .child(div().flex_1().min_w(px(16.)))
                .child(
                    div()
                        .id(SharedString::from(format!("tpl-del-{i}")))
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(24.))
                        .rounded(px(4.))
                        .text_color(t.muted)
                        .invisible()
                        .group_hover(group, |s| s.visible())
                        .hover(move |s| s.bg(hover))
                        .child("✕")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.delete_template(name.clone(), cx);
                        })),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.apply_template_style(style.clone(), window, cx)
                })),
        );
    }
    menu
}

pub fn small_button(
    id: &'static str,
    label: &'static str,
    primary: bool,
    t: &AppTheme,
) -> gpui::Stateful<Div> {
    let hover = t.hover;
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .h(px(30.))
        .px(px(10.))
        .rounded(px(6.))
        .text_size(px(13.))
        .when(primary, |d| d.bg(t.text).text_color(t.bg))
        .when(!primary, |d| {
            d.border_1()
                .border_color(t.border)
                .hover(move |s| s.bg(hover))
        })
        .child(label)
}

#[cfg(test)]
mod tests {
    use super::*;
    use xv_drawings::{Anchor, DrawingId, DrawingKind};

    #[test]
    fn template_keeps_text_content() {
        let a = Anchor {
            time_ms: 0,
            price: 1.0,
        };
        let mut d = Drawing::new(
            DrawingId(1),
            DrawingKind::TrendLine { a, b: a },
            Style::default(),
        );
        d.style.text.content = "keep me".into();
        let mut tpl = Style {
            width: 4,
            color: 0x112233FF,
            ..Style::default()
        };
        tpl.text.content = "template words".into();
        apply_template(&mut d, &tpl);
        assert_eq!(d.style.width, 4);
        assert_eq!(d.style.color, 0x112233FF);
        assert_eq!(d.style.text.content, "keep me");
        assert!(template_style(&tpl).text.content.is_empty());
    }
}
