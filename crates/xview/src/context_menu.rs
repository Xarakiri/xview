//! The drawing menu: the toolbar's More (⋯) popover and the right-click
//! context menu share these items; the context menu adds Lock/Unlock.

use std::rc::Rc;

use gpui::{App, Div, SharedString, Window, div, prelude::*, px};

use crate::theme::AppTheme;
use crate::widgets::{menu_box, menu_row};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MenuAction {
    Clone,
    Copy,
    Paste,
    Lock,
    Unlock,
    Hide,
    BringToFront,
    SendToBack,
    Settings,
    Delete,
}

/// Called with the picked action.
pub type OnPick = Rc<dyn Fn(MenuAction, &mut Window, &mut App)>;

#[derive(Clone, Debug, PartialEq)]
pub enum MenuItem {
    Item {
        action: MenuAction,
        label: &'static str,
        shortcut: Option<&'static str>,
        enabled: bool,
    },
    Separator,
}

fn item(action: MenuAction, label: &'static str, shortcut: Option<&'static str>) -> MenuItem {
    MenuItem::Item {
        action,
        label,
        shortcut,
        enabled: true,
    }
}

/// The menu for the selected drawing. `context` adds Lock/Unlock (the
/// right-click menu); Paste is disabled without a clipboard item.
pub fn items(context: bool, has_clipboard: bool, locked: bool) -> Vec<MenuItem> {
    let mut v = vec![
        item(MenuAction::Clone, "Clone", Some("⌘D")),
        item(MenuAction::Copy, "Copy", Some("⌘C")),
        MenuItem::Item {
            action: MenuAction::Paste,
            label: "Paste",
            shortcut: Some("⌘V"),
            enabled: has_clipboard,
        },
        MenuItem::Separator,
    ];
    if context {
        v.push(if locked {
            item(MenuAction::Unlock, "Unlock", None)
        } else {
            item(MenuAction::Lock, "Lock", None)
        });
    }
    v.extend([
        item(MenuAction::Hide, "Hide", None),
        item(MenuAction::BringToFront, "Bring to front", None),
        item(MenuAction::SendToBack, "Send to back", None),
        MenuItem::Separator,
        item(MenuAction::Settings, "Settings…", None),
        item(MenuAction::Delete, "Delete", Some("⌫")),
    ]);
    v
}

/// Renders `items` in a popover box; a click on an enabled row calls `pick`.
pub fn render(
    id_prefix: &str,
    items: &[MenuItem],
    theme: &AppTheme,
    pick: OnPick,
) -> Div {
    let mut menu = menu_box(theme).min_w(px(220.));
    for (i, it) in items.iter().enumerate() {
        menu = match *it {
            MenuItem::Separator => menu.child(div().h(px(1.)).mx(px(8.)).my(px(4.)).bg(theme.border)),
            MenuItem::Item {
                action,
                label,
                shortcut,
                enabled,
            } => {
                let pick = pick.clone();
                let row = menu_row(
                    SharedString::from(format!("{id_prefix}-{i}")),
                    None,
                    label,
                    false,
                    enabled,
                    theme,
                )
                .h(px(32.))
                .child(div().flex_1().min_w(px(24.)))
                .when_some(shortcut, |r, s| {
                    r.child(div().text_color(theme.muted).text_size(px(13.)).child(s))
                })
                .when(enabled, |r| r.on_click(move |_, window, cx| pick(action, window, cx)));
                menu.child(row)
            }
        };
    }
    menu
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actions(v: &[MenuItem]) -> Vec<Option<MenuAction>> {
        v.iter()
            .map(|i| match i {
                MenuItem::Item { action, .. } => Some(*action),
                MenuItem::Separator => None,
            })
            .collect()
    }

    #[test]
    fn more_menu_order_and_paste_state() {
        use MenuAction::*;
        let v = items(false, false, false);
        assert_eq!(
            actions(&v),
            vec![
                Some(Clone),
                Some(Copy),
                Some(Paste),
                None,
                Some(Hide),
                Some(BringToFront),
                Some(SendToBack),
                None,
                Some(Settings),
                Some(Delete)
            ]
        );
        let paste_enabled = |v: &[MenuItem]| {
            v.iter().any(|i| {
                matches!(
                    i,
                    MenuItem::Item {
                        action: Paste,
                        enabled: true,
                        ..
                    }
                )
            })
        };
        assert!(!paste_enabled(&v));
        assert!(paste_enabled(&items(false, true, false)));
    }

    #[test]
    fn context_menu_adds_lock_or_unlock() {
        let a = actions(&items(true, false, false));
        assert!(a.contains(&Some(MenuAction::Lock)) && !a.contains(&Some(MenuAction::Unlock)));
        let a = actions(&items(true, false, true));
        assert!(a.contains(&Some(MenuAction::Unlock)) && !a.contains(&Some(MenuAction::Lock)));
        assert_eq!(a.len(), items(false, false, false).len() + 1);
    }
}
