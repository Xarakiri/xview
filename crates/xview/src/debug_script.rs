//! Debug-only scripted input, for checking interactive flows without a
//! mouse (`XVIEW_SCRIPT`). Not used unless that variable is set.
//!
//! `XVIEW_SCRIPT` holds `;`-separated steps:
//! - `wait=MS`: sleep
//! - `tf=LABEL`: select a timeframe (`1m` … `M`)
//! - `search=TEXT`: open the symbol search with TEXT; `pick`: pick the
//!   selected result
//! - `pan=DX`: horizontal scroll of DX px over the chart
//! - `tool=NAME`: TrendLine, Ray, HorizontalLine, VerticalLine, Rectangle,
//!   Cursor; `click=X,Y`: click at element coordinates
//! - `csv=PATH`: open a CSV file as File → Open CSV… would
//! - `mods=cmd|shift|cmd+shift|none`: set the held modifiers (used by the
//!   pointer steps below) and forward the change to the chart
//! - `move=X,Y`, `down=X,Y`, `up=X,Y`: pointer move, left press, release
//!   (a move while pressed drags); `dblclick=X,Y`; `rclick=X,Y`
//! - `magnet=off|weak|strong`, `stay=on|off`: as the left toolbar would
//! - `style=dotted|dashed|solid|arrow|extend|stats|text:<s>|color:RRGGBBAA`:
//!   edit the selected drawing
//! - `flyout=magnet|eye|trend|none`: force a left-toolbar flyout open
//! - `log`: print the chart state to stderr
//! - `shot=NAME`: run `$XVIEW_SCRIPT_SHOT NAME` (e.g. a screenshot script)
//! - `quit`

use std::time::Duration;

use gpui::{Context, Window};
use xv_chart::{MagnetMode, Mods};
use xv_core::Timeframe;
use xv_drawings::{Extend, LineEnd, LineStyle, StatsMode, Tool};

use crate::workspace::Workspace;

pub fn script() -> Option<String> {
    std::env::var("XVIEW_SCRIPT").ok().filter(|s| !s.trim().is_empty())
}

/// Pointer state carried between steps.
#[derive(Default)]
struct Pointer {
    mods: Mods,
    pressed: bool,
}

fn xy(arg: &str) -> Option<(f32, f32)> {
    let mut it = arg.split(',').filter_map(|v| v.trim().parse::<f32>().ok());
    Some((it.next()?, it.next()?))
}

pub fn start(script: String, window: &Window, cx: &mut Context<Workspace>) {
    cx.spawn_in(window, async move |this, cx| {
        let mut ptr = Pointer::default();
        for step in script.split(';').map(str::trim).filter(|s| !s.is_empty()) {
            let (cmd, arg) = step.split_once('=').unwrap_or((step, ""));
            if cmd == "wait" {
                let ms = arg.parse().unwrap_or(0);
                cx.background_executor()
                    .timer(Duration::from_millis(ms))
                    .await;
                continue;
            }
            let (cmd, arg) = (cmd.to_string(), arg.to_string());
            if this
                .update_in(cx, |ws, window, cx| run(ws, &mut ptr, &cmd, &arg, window, cx))
                .is_err()
            {
                break;
            }
        }
    })
    .detach();
}

fn run(
    ws: &mut Workspace,
    ptr: &mut Pointer,
    cmd: &str,
    arg: &str,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    eprintln!("xview-script: {cmd}={arg}");
    match cmd {
        "mods" => {
            ptr.mods = Mods {
                shift: arg.contains("shift"),
                cmd: arg.contains("cmd"),
                alt: arg.contains("alt"),
            };
            let m = ptr.mods;
            ws.debug_chart(cx, |c, cx| {
                let fx = c.state.modifiers_changed(m);
                c.apply_effects(fx, cx);
            });
        }
        "move" | "down" | "up" | "dblclick" | "rclick" => {
            let Some(p) = xy(arg) else {
                eprintln!("xview-script: bad point {arg}");
                return;
            };
            let m = ptr.mods;
            if cmd == "down" {
                ptr.pressed = true;
            } else if cmd == "up" {
                ptr.pressed = false;
            }
            let pressed = ptr.pressed;
            ws.debug_chart(cx, |c, cx| {
                let fx = match cmd {
                    "move" => c.state.mouse_move(p, m, pressed),
                    "down" => c.state.mouse_down(p, m, 1),
                    "up" => c.state.mouse_up(p, m),
                    "dblclick" => {
                        let mut fx = c.state.mouse_move(p, m, false);
                        fx.extend(c.state.mouse_down(p, m, 1));
                        fx.extend(c.state.mouse_up(p, m));
                        fx.extend(c.state.mouse_down(p, m, 2));
                        fx.extend(c.state.mouse_up(p, m));
                        fx
                    }
                    _ => {
                        c.right_click(p, cx);
                        Vec::new()
                    }
                };
                c.apply_effects(fx, cx);
            });
        }
        "magnet" => match arg {
            "off" => ws.debug_set_magnet(false, None, cx),
            "weak" => ws.debug_set_magnet(true, Some(MagnetMode::Weak), cx),
            "strong" => ws.debug_set_magnet(true, Some(MagnetMode::Strong), cx),
            _ => eprintln!("xview-script: unknown magnet {arg}"),
        },
        "stay" => ws.debug_set_stay(arg == "on", cx),
        "flyout" => ws.debug_open_flyout(arg, cx),
        "style" => {
            let arg = arg.to_string();
            ws.debug_chart(cx, |c, cx| {
                let fx = c.state.update_selected(|d| {
                    let st = &mut d.style;
                    match arg.split_once(':') {
                        Some(("text", t)) => st.text.content = t.to_string(),
                        Some(("color", hex)) => match u32::from_str_radix(hex, 16) {
                            Ok(v) => st.color = v,
                            Err(_) => eprintln!("xview-script: bad color {hex}"),
                        },
                        _ => match arg.as_str() {
                            "solid" => st.line_style = LineStyle::Solid,
                            "dotted" => st.line_style = LineStyle::Dotted,
                            "dashed" => st.line_style = LineStyle::Dashed,
                            "arrow" => st.right_end = LineEnd::Arrow,
                            "extend" => st.extend = Extend::Right,
                            "stats" => st.stats = StatsMode::Values,
                            other => eprintln!("xview-script: unknown style {other}"),
                        },
                    }
                });
                c.apply_effects(fx, cx);
            });
        }
        "tf" => match Timeframe::ALL.iter().find(|t| t.label() == arg) {
            Some(tf) => ws.debug_select_timeframe(*tf, cx),
            None => eprintln!("xview-script: unknown timeframe {arg}"),
        },
        "csv" => ws.debug_open_csv(std::path::Path::new(arg), cx),
        "search" => ws.debug_open_search(arg.to_string(), window, cx),
        "pick" => ws.debug_pick_search(cx),
        "pan" => {
            let dx: f32 = arg.parse().unwrap_or(0.0);
            ws.debug_chart(cx, |c, cx| {
                let fx = c.state.scroll((400.0, 300.0), dx, 0.0, Mods::default());
                c.apply_effects(fx, cx);
            });
        }
        "tool" => {
            let tool = match arg {
                "TrendLine" => Tool::TrendLine,
                "ExtendedLine" => Tool::ExtendedLine,
                "Ray" => Tool::Ray,
                "HorizontalLine" => Tool::HorizontalLine,
                "VerticalLine" => Tool::VerticalLine,
                "Rectangle" => Tool::Rectangle,
                _ => Tool::Cursor,
            };
            ws.debug_chart(cx, |c, cx| {
                c.state.set_tool(tool);
                cx.notify();
            });
        }
        "click" => {
            let Some((x, y)) = xy(arg) else {
                return;
            };
            let m = ptr.mods;
            ws.debug_chart(cx, |c, cx| {
                let mut fx = c.state.mouse_move((x, y), m, false);
                fx.extend(c.state.mouse_down((x, y), m, 1));
                fx.extend(c.state.mouse_up((x, y), m));
                c.apply_effects(fx, cx);
            });
        }
        "log" => ws.debug_chart(cx, |c, _| {
            let s = &c.state;
            let cs = s.series.candles();
            let at = s.vp.index_of_x(400.0).round();
            let at_ms = (at >= 0.0)
                .then(|| cs.get(at as usize))
                .flatten()
                .map(|c| c.open_time_ms);
            eprintln!(
                "xview-script: log symbol={} tf={} len={} first={:?} last={:?} at_x400={:?} has_older={} loading_older={} magnet={} mode={:?} stay={} tool={:?} selected={:?} drawings={:?}",
                s.series.symbol.id,
                s.series.timeframe.label(),
                cs.len(),
                cs.first().map(|c| c.open_time_ms),
                cs.last().map(|c| (c.open_time_ms, c.close)),
                at_ms,
                s.series.has_older,
                s.loading_older,
                s.magnet,
                s.magnet_mode,
                s.stay_in_drawing_mode,
                s.tool,
                s.selected,
                s.drawings.all().iter().map(|d| (&d.kind, format!("{:08X}", d.style.color))).collect::<Vec<_>>(),
            );
        }),
        "shot" => {
            if let Some(cmd) = std::env::var_os("XVIEW_SCRIPT_SHOT") {
                let _ = std::process::Command::new(cmd).arg(arg).status();
            }
        }
        "quit" => cx.quit(),
        other => eprintln!("xview-script: unknown step {other}"),
    }
}
