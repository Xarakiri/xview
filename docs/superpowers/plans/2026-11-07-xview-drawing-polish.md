# Drawing Tools Polish Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make drawing feel exact and fast: ⌘ snaps to high/low, ⇧ snaps the angle, weak/strong magnet, stay-in-drawing-mode, a floating drawing toolbar, and a drawing settings dialog.

**Architecture:** Behaviour lives in the GPUI-free crates, tested with `cargo test`:
- `xv_drawings`: the model, file v2, set operations.
- `xv_chart`: snapping, state and frame primitives.
- `xv_store`: templates.

The `xview` binary adds only views: the toolbar, pickers, dialog and menus.

**Tech Stack:** Rust 2024, GPUI (pinned rev), serde_json.

**Spec:** docs/superpowers/specs/2026-11-07-xview-drawing-polish-design.md, which builds on docs/superpowers/specs/2026-11-07-xview-chart-core-design.md.

## Global Constraints

- macOS only. GPL-3.0-or-later. No TradingView logos, icon artwork or exact colors. The layout and behaviour follow the owner's reference.
- Anchors stay exact `time_ms` + `price`. Snapping and angle constraints change only where an anchor is placed. No screen round trip in storage.
- Drawings files written by M1 (`version: 1`) load unchanged. New fields use `#[serde(default)]`, and saves write `version: 2`. Older versions are migrated, never moved aside.
- No re-rendering of the shell on chart mouse moves. Popovers and the dialog are their own entities.
- Every new piece of logic has a unit test in its GPUI-free crate.
- Commits end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. The author is the repo-local identity. `.superpowers/` is never committed.
- Verification for interactive flows uses the debug-only `XVIEW_SCRIPT` hook (`crates/xview/src/debug_script.rs`). Extend it with new steps as needed. Screenshots capture the XView window only (`screencapture -l`).

## Review Focus

1. **Old files.** An M1 drawings file (v1, no new fields) loads, renders the same, and saves as v2 without loss. Test with a fixture JSON copied from the M1 format.
2. **Snap updates.** Pressing or releasing ⌘ with the mouse still updates the crosshair, the placement preview and an in-progress handle drag at once.
3. **⇧ at zero length.** ⇧ angle snap with the moving point at the fixed point (zero length) produces no NaN and no division by zero.
4. **Dialog Cancel.** Cancel after live edits restores the exact original drawing and leaves no undo step. Ok makes exactly one undo step.
5. **Visibility.** A drawing hidden by the Visibility settings on the current timeframe can't be hit-tested or selected, and its price labels don't show.

---

### Task 1: Drawing model v2

**Files:**
- Modify: `crates/xv_drawings/src/{model.rs, set.rs, tool.rs, file.rs, lib.rs}`
- Test: in-module tests, plus `crates/xv_drawings/tests/fixtures/m1_v1.json`

**Interfaces:**

Produces in `xv_drawings`:
- `enum LineStyle { Solid, Dashed, Dotted }` (default Solid)
- `enum Extend { None, Left, Right, Both }` (default None)
- `enum LineEnd { Normal, Arrow }`
- `enum StatsMode { Hidden, Compact, Values }`
- `enum StatsPos { Left, Center, Right }`
- `enum VAlign { Top, Middle, Bottom }`
- `enum HAlign { Left, Center, Right }`
- `struct TextStyle { content: String, color: u32, size: u8 /*10..=40, default 14*/, bold: bool, italic: bool, valign: VAlign /*Top*/, halign: HAlign /*Right*/ }`
- `struct Range { on: bool, min: u16, max: u16 }`
- `struct Visibility { minutes: Range /*1..=59*/, hours: Range /*1..=24*/, days: Range /*1..=366*/, weeks: Range /*1..=52*/, months: Range /*1..=12*/ }`. Its default has every range on and at full span.
  - `fn shows(&self, tf: Timeframe) -> bool`. M1..M30 map to minutes n, H1/H2/H4 to hours n, D1 to days 1, W1 to weeks 1, Mo1 to months 1.
  - `xv_drawings` gains an `xv_core` dependency if it lacks one.
- `Style`:
  - Keeps `color: u32` (RGBA, where alpha is the opacity) and `width: u8`.
  - Adds, all `#[serde(default)]`:
    - `line_style`
    - `fill: Option<u32>` (rectangle; `None` means the color at alpha 0x33, as in M1)
    - `extend`
    - `left_end`, `right_end`
    - `middle_point: bool`
    - `price_labels: bool`
    - `stats`, `stats_pos`, `always_stats: bool`
    - `text: TextStyle`
    - `visibility`
  - `Style` is no longer `Copy`.
- `Drawing` adds `#[serde(default)] name: Option<String>, locked: bool, hidden: bool`.
- `Tool` adds `ExtendedLine`.
  - `place_click` for `ExtendedLine` produces `DrawingKind::TrendLine`.
  - `fn key(self) -> &'static str` returns snake_case.
  - `fn default_style(self) -> Style`. ExtendedLine gets `extend: Both`. HorizontalLine gets `price_labels: true`. The rest are `Style::default()`.
- `fn tool_of(d: &Drawing) -> Tool`. A TrendLine with `extend == Both` is `ExtendedLine`.
- `DrawingSet` adds:
  - `fn update(&mut self, id, f: impl FnOnce(&mut Drawing)) -> bool`: one undo step; rejects a non-finite result.
  - `fn bring_to_front(&mut self, id)` and `fn send_to_back(&mut self, id)`: one undo step each.
  - `fn insert_copy(&mut self, d: &Drawing) -> Option<DrawingId>`: new id, one undo step.
  - `fn snapshot(&self, id) -> Option<Drawing>`
  - `fn restore_without_undo(&mut self, d: Drawing)`: used by the dialog's live preview and Cancel.
- `file.rs`: `FILE_VERSION = 2`. `from_json` accepts versions 1 and 2 and rejects anything greater (existing error). `to_json` writes 2.

- [ ] **Step 1: Write failing tests**
  - `v1_file_loads_with_defaults`: the fixture is a v1 JSON with one of each kind, written in the exact M1 format.
    - It parses.
    - Every style equals the M1 color/width with the new fields at their defaults.
    - The rectangle has `fill == None`.
    - `to_json` then `from_json` round-trips with `version: 2`.
  - `visibility_shows_maps_timeframes`:
    - With minutes 5..=15, M1 is false and M5, M15 are true.
    - Hours off makes H1 false.
    - The default shows all 11 timeframes.
  - `update_is_one_undo_step`: update the color and width, then undo restores both.
  - `update_rejects_non_finite`.
  - `z_order_ops`: front and back reorder `all()`, and each one undoes.
  - `insert_copy_new_id`.
  - `restore_without_undo_leaves_history`.
  - `extended_line_tool_places_trendline` and `tool_of_roundtrip`.
- [ ] **Step 2:** Run `cargo test -p xv_drawings`. They fail.
- [ ] **Step 3:** Implement. Fix the compile errors across the workspace that come from `Style` no longer being `Copy`. Use `.clone()` where needed and keep behaviour the same.
- [ ] **Step 4:** Run `cargo test --workspace`. All pass.
- [ ] **Step 5:** Commit `feat(drawings): style v2, visibility, z-order, file v2 with v1 migration`.

---

### Task 2: Snapping, magnet modes, angle constraint, state API

**Files:**
- Modify: `crates/xv_chart/src/{snap.rs, state.rs, lib.rs}`

**Interfaces:**

Consumes Task 1.

Produces in `xv_chart`:
- `enum MagnetMode { Weak, Strong }`
- `const MAGNET_WEAK_PX: f32 = 12.0`
- `snap.rs`:
  - `fn snap_high_low(candles, tf, vp, p) -> Anchor`: nearest bar by x, then the nearer of High or Low.
  - `fn snap_ohlc(candles, tf, vp, p, max_px: Option<f32>) -> Option<Anchor>`: `None` when the nearest level is more than `max_px` away vertically. The existing `snap_anchor` becomes `snap_ohlc(.., None).unwrap_or(raw)`.
  - `fn constrain_angle(fixed: (f32,f32), moving: (f32,f32)) -> (f32,f32)`: projects onto the nearest multiple of 45° in screen space. With zero length it returns `moving` unchanged.
  - `fn constrain_square(fixed, moving) -> (f32,f32)`.
- `ChartState`:
  - `pub magnet: bool` stays the on/off switch. Add `pub magnet_mode: MagnetMode` (default Weak) and `pub stay_in_drawing_mode: bool`. This removes "⇧ keeps tool".
  - Snap precedence for any anchor under the pointer:
    - With ⇧ while a second point or endpoint is moving: an angle or square constraint is applied in screen space to the raw point, then converted.
    - Otherwise ⌘ gives `snap_high_low`.
    - Otherwise, magnet on with Strong gives OHLC.
    - Otherwise, magnet on with Weak gives OHLC within 12 px.
    - Otherwise the raw point.
  - Rule: this applies to the placement preview, the placement clicks, handle drags of two-point drawings (the fixed point is the other anchor), and hover with a drawing tool or ⌘ held.
  - Body drags are not snapped.
  - `pub fn modifiers_changed(&mut self, m: Mods) -> Vec<ChartEffect>`: re-runs the current pointer position with the new modifiers, for the crosshair, the preview and an active handle drag.
  - The crosshair shown in `frame()` is the snapped point while a drawing tool is active or ⌘ is held. Store the snapped element point alongside the raw one.
  - Per-drawing `locked`:
    - It can be selected, but body and handle drags don't start.
    - Delete still works, since lock means "don't move by accident" (my call, recorded).
    - A globally locked state (`drawings_locked`) keeps its M1 meaning.
  - Per-drawing `hidden`, and `!style.visibility.shows(series.timeframe)`: not hit-testable, not drawn, deselected if selected.
  - Double-click on a drawing in Cursor mode emits `ChartEffect::OpenSettings(DrawingId)` and selects it.
  - `pub fn select_at(&mut self, p) -> Option<DrawingId>`: for right-click. It selects the topmost drawing under `p` and returns it, or deselects and returns None.
  - `pub fn selected_drawing(&self) -> Option<&Drawing>`
  - `pub fn update_selected(&mut self, f) -> Vec<ChartEffect>`: one undo step. It records the style as last-used for `tool_of(drawing)` and emits `DrawingsChanged` plus `ToolStylesChanged`.
  - `pub fn begin_edit(&mut self) -> Option<Drawing>`, `pub fn preview_edit(&mut self, d: Drawing) -> Vec<ChartEffect>` (no undo), `pub fn commit_edit(&mut self, original: Drawing) -> Vec<ChartEffect>` (one undo step from original to current), `pub fn cancel_edit(&mut self, original: Drawing) -> Vec<ChartEffect>` (restore, no undo).
  - Selection operations, each one undo step where it mutates:
    - `pub fn clone_selected(&mut self) -> Vec<ChartEffect>`: offset +10 px right and down via `shift_anchor`.
    - `pub fn paste(&mut self, d: &Drawing) -> Vec<ChartEffect>`: same anchors, new id, selected.
    - `bring_selected_to_front`, `send_selected_to_back`, `set_selected_locked(bool)`, `hide_selected`, `unhide_all() -> Vec<ChartEffect>`, `hidden_count() -> usize`.
  - `pub tool_styles: HashMap<Tool, Style>`. New drawings use `tool_styles.get(tool).cloned().unwrap_or(tool.default_style())`. `set_selected_style` keeps working (color/width) through `update_selected`.
  - `ChartEffect` adds `OpenSettings(DrawingId)` and `ToolStylesChanged`.

- [ ] **Step 1: Write failing tests**
  - `cmd_snaps_to_high_or_low_only`: the cursor is nearest the close level but above the midpoint, and the snap picks High.
  - `weak_magnet_threshold`: a level 8 px away snaps, one 20 px away doesn't.
  - `strong_magnet_always_snaps`.
  - `cmd_overrides_weak_magnet`.
  - `constrain_angle_cases`: 10° goes to 0°, 40° to 45°, 80° to 90°, 170° to 180°, negative angles work, and zero length doesn't panic.
  - `shift_constrains_second_point` and `shift_constrains_handle_drag`: the fixed anchor is unchanged, and the result lies on a 45° multiple in screen space.
  - `rectangle_shift_is_square`.
  - `modifiers_changed_updates_preview_and_crosshair`: no mouse move between the calls.
  - `stay_in_drawing_mode_keeps_tool`, and `shift_no_longer_keeps_tool`. Replace the old `shift_keeps_tool` test.
  - `locked_drawing_selectable_not_draggable`.
  - `visibility_hidden_not_hittable_and_deselects_on_tf_change`.
  - `double_click_emits_open_settings`.
  - `edit_cancel_restores_without_undo` and `edit_commit_one_undo_step`.
  - `clone_paste_zorder_hide_unhide`.
  - `new_drawing_uses_tool_style` and `update_selected_records_last_used`.
- [ ] **Step 2:** Run `cargo test -p xv_chart`. They fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run `cargo test --workspace`. All pass.
- [ ] **Step 5:** Commit `feat(chart): cmd high/low snap, shift angle snap, magnet modes, edit API`.

---

### Task 3: Frame rendering of new style options

**Files:**
- Modify: `crates/xv_chart/src/frame.rs`, and `ticks.rs` if formatting helpers are needed.

**Interfaces:**

Produces:
- `Prim::Line { a, b, color, width, dash: Dash }`. `enum Dash { Solid, Dashed, Dotted }` replaces `dashed: bool`. Update every builder.
- `Prim::Polygon { pts: Vec<(f32,f32)>, color: u32 }`: filled, for arrowheads.
- `Prim::Text` adds `size: f32`, `bold: bool`, `italic: bool`. Existing callers pass 12 / false / false.
- Rendering rules (all clipped to the plot, which already happens):
  - **Line style:** dash and opacity come from the color's alpha.
  - **Extend:** Left, Right and Both extend a trend line to the plot edges along its direction. A ray keeps "extends from a through b".
  - **Arrow ends:** an 8×10 px filled triangle at that end, pointing outward.
  - **Middle point:** a 3 px radius dot at the segment midpoint.
  - **Price labels:** each anchor's price goes on the price axis as a label, with the drawing color as background and contrasting text. They are drawn only when the drawing is visible.
  - **Stats:**
    - Compact: `+12.34 (+1.23%)`.
    - Values: compact plus bar count, time span (`3d 4h`) and angle in screen degrees.
    - They sit near the b-end (Right), the midpoint (Center) or the a-end (Left), shown when `always_stats` is set or the drawing is selected.
    - For a rectangle: the price range, %, and bars.
  - **Drawing text:**
    - It is anchored relative to the segment's bounding box: valign picks above, centred or below, halign picks a-end, middle or b-end.
    - Unrotated, multi-line (one `Prim::Text` per line), size, bold and italic.
    - H-line text sits along the line. V-line text sits along the line near the top.
  - **Rectangle:** fill is `fill.unwrap_or(color with alpha 0x33)`. Extend Left/Right extends the fill and border horizontally to the plot edge.
  - **Visibility and hidden:** such drawings are skipped entirely.
  - Selected drawings still show handles. Locked selected drawings show handles hollow (handle color with alpha 0x80).

- [ ] **Step 1: Write failing tests**
  - `dash_variants_emitted`.
  - `extend_both_reaches_plot_edges`.
  - `arrow_end_emits_polygon`.
  - `middle_point_dot`.
  - `price_labels_on_axis`.
  - `stats_compact_text`, with exact string.
  - `text_lines_and_alignment`.
  - `rect_fill_default_and_custom`.
  - `visibility_skips_drawing`.
  - `non_finite_never_emitted` (existing guard still holds).
- [ ] **Step 2:** Run `cargo test -p xv_chart frame`. They fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run `cargo test --workspace`. All pass.
- [ ] **Step 5:** Commit `feat(chart): render line styles, extend, arrows, labels, stats, text`.

---

### Task 4: Templates store and settings fields

**Files:**
- Create: `crates/xv_store/src/templates.rs`
- Modify: `crates/xv_store/src/{settings.rs, lib.rs}`

**Interfaces:**

Produces:
- `struct TemplatesFile { version: u32 /*1*/, last_used: BTreeMap<String /*Tool::key*/, Style>, named: BTreeMap<String /*Tool::key*/, Vec<NamedTemplate>>, custom_colors: Vec<u32> /*max 10, newest first*/ }`
- `struct NamedTemplate { name: String, style: Style }`
- `fn templates_path(dir) -> PathBuf` (`templates.json`)
- `fn load_templates(dir) -> LoadedTemplates { Ok(TemplatesFile), Missing, Corrupt{path, moved} }`
- `fn save_templates(dir, &TemplatesFile) -> io::Result<()>`. Same `write_atomic`, refuse-if-unparseable and move-aside rules as drawings.
- `Settings` adds `#[serde(default)] magnet_on: bool, magnet_mode: MagnetSetting { Weak, Strong } /*default Weak*/, stay_in_drawing_mode: bool`. `xv_store` must not depend on `xv_chart`, so the host maps `MagnetSetting` to `MagnetMode`.

- [ ] **Step 1: Write failing tests**
  - `templates_roundtrip`.
  - `templates_corrupt_moved_aside`.
  - `save_refuses_over_unparseable`.
  - `settings_v1_without_new_fields_loads_defaults`.
  - `custom_colors_capped_at_10` (helper `push_custom_color`).
- [ ] **Step 2:** Run `cargo test -p xv_store`. They fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run `cargo test --workspace`. All pass.
- [ ] **Step 5:** Commit `feat(store): drawing templates file and magnet settings`.

---

### Task 5: App wiring: painting, modifiers, left toolbar, persistence

**Files:**
- Modify: `crates/xview/src/{chart_view.rs, left_toolbar.rs, workspace.rs, persistence.rs, assets.rs, debug_script.rs}`
- Create: icons `magnet_weak.svg`, `magnet_strong.svg` (with small lightning marks, original), `pencil_lock.svg`, and entries in `LICENSE-icons.md`.

**Interfaces:**

Consumes Tasks 1–4.

Produces:
- `ChartView` paints:
  - `Dash::Dotted` (dash array 1×3 px with round caps) and `Dash::Dashed` (6×4).
  - `Prim::Polygon` (`PathBuilder::fill`).
  - Text with size, bold (semibold) and italic. Italic uses `FontStyle::Italic`.
- `ChartView` forwards `ModifiersChangedEvent` to `state.modifiers_changed`.
- Right mouse down calls `state.select_at` and emits `ChartViewEvent::ContextMenu { id: DrawingId, position: Point<Pixels> }` (window coordinates).
- `ChartEffect::OpenSettings(id)` maps to `ChartViewEvent::OpenSettings(id)`.
- `ToolStylesChanged` maps to `ChartViewEvent::ToolStylesChanged`.
- Actions in the `ChartView` key context: `Copy` (cmd-c), `Paste` (cmd-v), `Clone` (cmd-d, my call).
- Left toolbar:
  - The magnet button toggles `magnet`, and its icon reflects the mode (weak or strong). It has a flyout with "Weak magnet", "Strong magnet" (a check on the current one) and a separator.
  - "Snap to indicators" is a disabled toggle with the tooltip "Coming soon".
  - A new **Stay in drawing mode** button (`pencil_lock`) goes between the magnet and the lock. It toggles `stay_in_drawing_mode` and shows as active when on.
  - The eye button gets a flyout: "Hide all drawings" (toggle, the existing behaviour) and "Show N hidden drawings" (calls `unhide_all`, disabled when N is 0).
  - In the trend-line group flyout, "Extended line" is now enabled (`Tool::ExtendedLine`).
- Workspace and persistence:
  - At startup, load templates and settings. Set `state.tool_styles` from `last_used`, and set magnet and stay-mode from settings.
  - On `ToolStylesChanged`, save templates on a 500 ms debounce. Do it in the background, the way drawings are saved, and show a banner on error.
  - Save settings when magnet or stay-mode changes.
  - Copy and Paste use an in-app clipboard (`Option<Drawing>` on Workspace). Paste is a no-op when the clipboard is empty, and it works only within the current symbol (the clipboard is cleared on symbol switch, my call).
- `debug_script` gains steps for verification:
  - `mods=cmd|shift|none` (calls `modifiers_changed`)
  - `move=X,Y` (`mouse_move`)
  - `down=X,Y`, `up=X,Y`
  - `dblclick=X,Y`
  - `rclick=X,Y`
  - `magnet=off|weak|strong`
  - `stay=on|off`

- [ ] **Step 1:** Implement.
- [ ] **Step 2:** Run `cargo build -p xview`, `cargo clippy --workspace -- -D warnings` and `cargo test --workspace`.
- [ ] **Step 3: Scripted check** with `XVIEW_DEMO=1` (deterministic) in a debug build. Take window screenshots of:
  - (a) the ⌘ crosshair snapping to a High;
  - (b) a ⇧ 45° trend line;
  - (c) the magnet flyout (forced open by a debug env var, or a click step if one is added);
  - (d) dotted and dashed lines and an arrow end. Set these via a debug step `style=dotted|dashed|arrow` on the selected drawing, or via unit-level checks.
- [ ] **Step 4:** Commit `feat(app): snapping keys, magnet flyout, stay-in-drawing mode, templates persistence`.

---

### Task 6: Floating drawing toolbar, color picker, menus

**Files:**
- Create: `crates/xview/src/drawing_toolbar.rs` (replaces `style_bar.rs`; delete it), `crates/xview/src/color_picker.rs`, `crates/xview/src/context_menu.rs`
- Modify: `workspace.rs`, `widgets.rs` (shared popover, menu row, separator), `assets.rs`
- Create: icons `templates.svg`, `pencil.svg`, `text_color.svg`, `settings_hex.svg`, `alarm_plus.svg`, `lock_open.svg`, `grip.svg`, `more.svg` (reuse `more` if one exists), and their license entries.

**Interfaces:**

Consumes the Task 2 API and Task 5 events.

Produces the floating toolbar for the selected drawing:
- **Position:** 8 px above the selection bounds (`selected_screen_bounds`), or below if there's no room. It stays clamped inside the chart area. After the grip is dragged, the dragged offset is kept until the selection clears, and is reset on the next selection.
- **Layout:** a white card with a shadow and radius 8, 52 px tall, and 44×44 buttons. The order is exactly spec §2.3.
- **Color buttons:** the icon has a 4 px color bar under it, showing the current line color or text color.
- **Width button:** a short line glyph plus "Npx".
- **Disabled or hidden buttons per type:**
  - V-line and H-line have no text-color button.
  - All types have text.
  - Alert is always disabled ("Alerts (coming soon)").
- **Popovers** (one open at a time; Esc or a click outside closes them):
  - **Templates:**
    - "Save Drawing Template As…" opens a small name input with Save/Cancel, which adds to `named[tool]`.
    - "Apply Default Drawing Template" applies `tool.default_style()`.
    - Then each named template for the tool, with a hover ✕ to delete it.
    - Applying a template keeps the drawing's text content, and replaces the style only.
  - **Color picker** (`color_picker.rs`, reused by the dialog):
    - A 10×8 grid: row 1 grays from white to black, row 2 saturated hues, rows 3–8 tints and shades of the row-2 hues from light to dark. Pick XView's own 10 hues, distinct from the M1 palette's first color.
    - A divider, then recent custom colors and a "+" that opens a hex input (`#RRGGBB`) and pushes onto `custom_colors`.
    - An "Opacity" slider (0–100) with a % field. It changes alpha only.
    - The selected swatch gets a 2 px dark ring.
    - Changes apply live through `update_selected`. A whole picker session should be one undo step: call `begin_edit` on open, `preview_edit` while it's open, and `commit_edit` on close.
  - **Width:** 1, 2, 3 and 4 px rows, each with a line sample. The current one is highlighted with a dark background and white text, as in the reference.
  - **Line style:** Line, Dashed line, Dotted line rows, each with a sample.
  - **More (⋯):** Clone, Copy, Paste (disabled when the clipboard is empty), a separator, Hide, Bring to front, Send to back, a separator, Settings…, Delete.
- **Lock button:** toggles the drawing's `locked`. The icon is closed when locked and open when not.
- **Delete button:** deletes the drawing.
- **Settings button:** opens the dialog (Task 7). Until Task 7 lands, it emits the event and logs.
- **Right-click context menu** (`context_menu.rs`): appears at the click position with the same items as More, plus Lock/Unlock. Right-clicking empty chart space opens nothing in this task.
- **Popover clicks never reach the chart:** use occlude plus `stop_propagation`, as the M1 style bar did.

- [ ] **Step 1:** Unit-test the pure helpers:
  - `palette_grid_10x8_unique`
  - `hex_parse_roundtrip` (accepts `#abc`? No: only 6 digits, case-insensitive)
  - `opacity_sets_alpha_only`
  - `toolbar_position_flip_clamp` (move the M1 style-bar position tests over)
- [ ] **Step 2:** Implement.
- [ ] **Step 3:** Build, clippy and test.
- [ ] **Step 4:** Take window screenshots, with popovers forced open through a debug env var or script steps, of the toolbar and each popover. Compare them against the reference images, which the controller provides in the dispatch.
- [ ] **Step 5:** Commit `feat(app): floating drawing toolbar, color picker, templates and context menus`.

---

### Task 7: Drawing settings dialog

**Files:**
- Create: `crates/xview/src/drawing_settings.rs`, `crates/xview/src/form.rs` (checkbox, select dropdown, number field with steppers, text field, text area, tabs, dual-handle range slider, date-time field)
- Modify: `workspace.rs`, `symbol_search.rs` (move its single-line input into `form.rs` and reuse it)

**Interfaces:**

Consumes Tasks 2, 4 and 6.

Produces a modal card, 400 px wide with radius 16, centred over the chart:
- **Header:** the name (or the tool's display name) at 24 px semibold, with a pencil that switches the name to an inline text field. Enter or blur commits it to `Drawing.name`. There is a ✕ close button, which acts as Cancel.
- **Tabs:** Style, Text, Coordinates, Visibility. The active tab is underlined. Rows follow spec §2.4 and the per-type table in §2.5.
- **Style rows:**
  - Line: a color swatch that opens `color_picker` in a popover, plus a style+width button that opens a small popover combining the line-style and width lists.
  - Left end and right end: icon buttons that open a Normal/Arrow menu.
  - Extend: a select.
  - Middle point and Price labels: checkboxes.
  - An "INFO" caption, then Stats (select), Stats position (select) and Always show stats (checkbox).
  - Rectangle: Border and Fill rows, each with a swatch and opacity.
- **Text tab:**
  - A color swatch, a size select (10, 11, 12, 14, 16, 20, 24, 28, 32, 40), and B and I toggles.
  - A text area (min 4 lines) with placeholder "Add text". Enter inserts a newline.
  - Text alignment: a vertical select and a horizontal select.
- **Coordinates tab:**
  - One row per anchor: "#1 (price, time)". A price number field (steppers move by one tick size) and a date-time field `YYYY-MM-DD HH:MM` in the display tz.
  - An invalid input gets a red border and isn't applied.
  - H-line has price only. V-line has time only.
- **Visibility tab:**
  - Rows for Minutes, Hours, Days, Weeks and Months. Each row has a checkbox, a min field, a dual-handle slider and a max field, bounded per Task 1.
  - Min is clamped to max or below.
- **Footer:**
  - A Template ▾ select, with the same items as the toolbar's templates menu. It applies to the dialog's working copy.
  - Cancel, which calls `cancel_edit(original)`.
  - Ok, dark filled, which calls `commit_edit(original)`.
- **Behaviour:**
  - Every change previews live on the chart through `preview_edit`.
  - Esc means Cancel. ⌘Enter means Ok.
  - Opening captures `original = begin_edit()`.
  - While the dialog is open the chart doesn't take keyboard input. The dialog's focused field has focus, and chart focus is restored on close.
  - Opened by: a double-click on a drawing, the toolbar Settings button, More → Settings…, or the context menu Settings….

- [ ] **Step 1:** Unit-test the pure helpers in `form.rs` and `drawing_settings.rs`:
  - `datetime_parse_format_roundtrip_with_tz`
  - `price_step_uses_tick`
  - `range_slider_clamps`
  - `rows_for_kind` (which rows each kind shows)
- [ ] **Step 2:** Implement.
- [ ] **Step 3:** Build, clippy and test.
- [ ] **Step 4:** Take scripted window screenshots of each tab for a trend line, and of the Style tab for a rectangle and an H-line. Then check the scripted flows:
  - Cancel after a change restores the drawing, with `log` showing no undo step.
  - Ok makes exactly one undo step.
- [ ] **Step 5:** Commit `feat(app): drawing settings dialog with style, text, coordinates, visibility`.

---

### Task 8: README and final verification

**Files:**
- Modify: `README.md`

- [ ] **Step 1:** In README "Using it", document:
  - ⌘ snapping to high/low, and ⇧ for 45° angles and squares;
  - the weak/strong magnet;
  - Stay in drawing mode;
  - the drawing toolbar, right-click menu and settings dialog (double-click);
  - ⌘C, ⌘V and ⌘D;
  - templates.

  Remove the old "⇧-click keeps the tool" text if it exists.
- [ ] **Step 2:** Run `cargo test --workspace` (all pass), `cargo clippy --workspace -- -D warnings` (clean) and `cargo build -p xview --release`.
- [ ] **Step 3:** Commit `docs: drawing tools in README`.
