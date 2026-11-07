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
//! - `log`: print the chart state to stderr
//! - `shot=NAME`: run `$XVIEW_SCRIPT_SHOT NAME` (e.g. a screenshot script)
//! - `quit`

use std::time::Duration;

use gpui::{Context, Window};
use xv_chart::Mods;
use xv_core::Timeframe;
use xv_drawings::Tool;

use crate::workspace::Workspace;

pub fn script() -> Option<String> {
    std::env::var("XVIEW_SCRIPT").ok().filter(|s| !s.trim().is_empty())
}

pub fn start(script: String, window: &Window, cx: &mut Context<Workspace>) {
    cx.spawn_in(window, async move |this, cx| {
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
                .update_in(cx, |ws, window, cx| run(ws, &cmd, &arg, window, cx))
                .is_err()
            {
                break;
            }
        }
    })
    .detach();
}

fn run(ws: &mut Workspace, cmd: &str, arg: &str, window: &mut Window, cx: &mut Context<Workspace>) {
    eprintln!("xview-script: {cmd}={arg}");
    match cmd {
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
            let mut it = arg.split(',').filter_map(|v| v.trim().parse::<f32>().ok());
            let (Some(x), Some(y)) = (it.next(), it.next()) else {
                return;
            };
            ws.debug_chart(cx, |c, cx| {
                let m = Mods::default();
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
                "xview-script: log symbol={} tf={} len={} first={:?} last={:?} at_x400={:?} has_older={} loading_older={} drawings={:?}",
                s.series.symbol.id,
                s.series.timeframe.label(),
                cs.len(),
                cs.first().map(|c| c.open_time_ms),
                cs.last().map(|c| (c.open_time_ms, c.close)),
                at_ms,
                s.series.has_older,
                s.loading_older,
                s.drawings.all().iter().map(|d| &d.kind).collect::<Vec<_>>(),
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
