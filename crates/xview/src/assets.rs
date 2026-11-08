//! Embedded assets: the SVG icons under `assets/icons`, served to GPUI as
//! `icons/<name>.svg`.

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

/// Every embedded icon name (without directory or extension).
pub const ICON_NAMES: &[&str] = &[
    "alarm_plus",
    "avatar_placeholder",
    "bars_style",
    "bell",
    "brush",
    "brush_shapes",
    "bulb",
    "calendar",
    "candles",
    "chat",
    "chevron_down",
    "crosshair",
    "emoji",
    "extended_line",
    "eye",
    "fibonacci",
    "grip",
    "help",
    "horizontal_line",
    "horizontal_lines",
    "horizontal_ray",
    "list",
    "lock",
    "lock_open",
    "magnet",
    "magnet_strong",
    "magnet_weak",
    "measure",
    "more",
    "patterns",
    "pencil",
    "pencil_lock",
    "plus",
    "plus_circle",
    "ray",
    "rectangle",
    "settings",
    "settings_hex",
    "templates",
    "text",
    "text_color",
    "trash",
    "trend_line",
    "vertical_line",
    "zoom",
];

fn icon_bytes(name: &str) -> Option<&'static [u8]> {
    Some(match name {
        "alarm_plus" => include_bytes!("../assets/icons/alarm_plus.svg"),
        "avatar_placeholder" => include_bytes!("../assets/icons/avatar_placeholder.svg"),
        "bars_style" => include_bytes!("../assets/icons/bars_style.svg"),
        "bell" => include_bytes!("../assets/icons/bell.svg"),
        "brush" => include_bytes!("../assets/icons/brush.svg"),
        "brush_shapes" => include_bytes!("../assets/icons/brush_shapes.svg"),
        "bulb" => include_bytes!("../assets/icons/bulb.svg"),
        "calendar" => include_bytes!("../assets/icons/calendar.svg"),
        "candles" => include_bytes!("../assets/icons/candles.svg"),
        "chat" => include_bytes!("../assets/icons/chat.svg"),
        "chevron_down" => include_bytes!("../assets/icons/chevron_down.svg"),
        "crosshair" => include_bytes!("../assets/icons/crosshair.svg"),
        "emoji" => include_bytes!("../assets/icons/emoji.svg"),
        "extended_line" => include_bytes!("../assets/icons/extended_line.svg"),
        "eye" => include_bytes!("../assets/icons/eye.svg"),
        "fibonacci" => include_bytes!("../assets/icons/fibonacci.svg"),
        "grip" => include_bytes!("../assets/icons/grip.svg"),
        "help" => include_bytes!("../assets/icons/help.svg"),
        "horizontal_line" => include_bytes!("../assets/icons/horizontal_line.svg"),
        "horizontal_lines" => include_bytes!("../assets/icons/horizontal_lines.svg"),
        "horizontal_ray" => include_bytes!("../assets/icons/horizontal_ray.svg"),
        "list" => include_bytes!("../assets/icons/list.svg"),
        "lock" => include_bytes!("../assets/icons/lock.svg"),
        "lock_open" => include_bytes!("../assets/icons/lock_open.svg"),
        "magnet" => include_bytes!("../assets/icons/magnet.svg"),
        "magnet_strong" => include_bytes!("../assets/icons/magnet_strong.svg"),
        "magnet_weak" => include_bytes!("../assets/icons/magnet_weak.svg"),
        "measure" => include_bytes!("../assets/icons/measure.svg"),
        "more" => include_bytes!("../assets/icons/more.svg"),
        "patterns" => include_bytes!("../assets/icons/patterns.svg"),
        "pencil" => include_bytes!("../assets/icons/pencil.svg"),
        "pencil_lock" => include_bytes!("../assets/icons/pencil_lock.svg"),
        "plus" => include_bytes!("../assets/icons/plus.svg"),
        "plus_circle" => include_bytes!("../assets/icons/plus_circle.svg"),
        "ray" => include_bytes!("../assets/icons/ray.svg"),
        "rectangle" => include_bytes!("../assets/icons/rectangle.svg"),
        "settings" => include_bytes!("../assets/icons/settings.svg"),
        "settings_hex" => include_bytes!("../assets/icons/settings_hex.svg"),
        "templates" => include_bytes!("../assets/icons/templates.svg"),
        "text" => include_bytes!("../assets/icons/text.svg"),
        "text_color" => include_bytes!("../assets/icons/text_color.svg"),
        "trash" => include_bytes!("../assets/icons/trash.svg"),
        "trend_line" => include_bytes!("../assets/icons/trend_line.svg"),
        "vertical_line" => include_bytes!("../assets/icons/vertical_line.svg"),
        "zoom" => include_bytes!("../assets/icons/zoom.svg"),
        _ => return None,
    })
}

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(path
            .strip_prefix("icons/")
            .and_then(|p| p.strip_suffix(".svg"))
            .and_then(icon_bytes)
            .map(Cow::Borrowed))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        if path.trim_end_matches('/') == "icons" {
            Ok(ICON_NAMES
                .iter()
                .map(|n| SharedString::from(format!("icons/{n}.svg")))
                .collect())
        } else {
            Ok(Vec::new())
        }
    }
}

/// Asset path of an icon, e.g. `icon_path("trash") == "icons/trash.svg"`.
pub fn icon_path(name: &str) -> SharedString {
    format!("icons/{name}.svg").into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_loads_and_is_28_viewbox() {
        for n in ICON_NAMES {
            let data = Assets.load(&icon_path(n)).unwrap().expect(n);
            let s = std::str::from_utf8(&data).unwrap();
            assert!(s.contains(r#"viewBox="0 0 28 28""#), "{n}");
            assert!(s.contains(r#"stroke-width="1.5""#), "{n}");
            assert!(s.contains("currentColor"), "{n}");
        }
        assert!(Assets.load("icons/nope.svg").unwrap().is_none());
        assert_eq!(Assets.list("icons").unwrap().len(), ICON_NAMES.len());
    }

    #[test]
    fn brief_icon_names_exist() {
        for n in [
            "avatar_placeholder",
            "candles",
            "bars_style",
            "plus_circle",
            "chevron_down",
            "crosshair",
            "trend_line",
            "horizontal_lines",
            "patterns",
            "fibonacci",
            "brush_shapes",
            "text",
            "emoji",
            "measure",
            "zoom",
            "magnet",
            "magnet_weak",
            "magnet_strong",
            "pencil_lock",
            "lock",
            "eye",
            "trash",
            "ray",
            "extended_line",
            "horizontal_line",
            "horizontal_ray",
            "vertical_line",
            "rectangle",
            "calendar",
            "settings",
        ] {
            assert!(ICON_NAMES.contains(&n), "{n}");
        }
    }
}
