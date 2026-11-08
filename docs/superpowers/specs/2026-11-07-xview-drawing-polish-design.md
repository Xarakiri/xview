# XView — Drawing tools polish (design)

Date: 2026-11-07
Status: ready for owner review
Builds on: `2026-11-07-xview-chart-core-design.md` (Milestone 1). Branch `drawing-polish`, stacked on `m1-chart-core`.

## 1. Intent

The owner draws lines and zones all day, so drawing has to feel exact and quick. This round adds three things:
- precise snapping, using the magnet and two modifier keys;
- a floating toolbar for the selected drawing;
- a settings dialog for the selected drawing.

They follow the owner's reference screenshots (2026-11-07) for behaviour, layout and contents. As always, the icons, colors and branding are XView's own.

## 2. Goals

### 2.1 Snapping

- **⌘ held:** the anchor and the crosshair snap to the nearest candle's **High or Low**. The nearest candle is picked by x; between its High and Low, the one nearer the cursor wins. This works while placing, while dragging a handle, and while just hovering with a drawing tool. Pressing or releasing ⌘ updates the snap at once, without waiting for the mouse to move.
- **⇧ held:** while placing the second point or dragging an endpoint of a two-point drawing, the line is constrained to the nearest multiple of 45° in screen space: 0°, 45°, 90°, 135° and so on.
  - The fixed point stays where it is.
  - The moving point is projected onto the snapped direction.
  - Rectangle with ⇧: constrained to a square, measured in pixels *(my call)*.
  - ⇧ and ⌘ together: ⌘ picks the price and ⇧ picks the direction. The point is placed at the cursor's bar, on the snapped direction *(my call: angle wins for the price; ⌘ only applies when ⇧ is not held)*.
- **Magnet modes.** The left-toolbar magnet button gets a flyout like the reference:
  - **Weak magnet:** snaps to the nearest O/H/L/C of the bar under the cursor, but only when that level is within **12 px** vertically *(my call)*. Otherwise the point is placed freely.
  - **Strong magnet:** always snaps to the nearest O/H/L/C.
  - **"Snap to indicators":** shown disabled ("Coming soon"), since there are no indicators yet.
  - Clicking the main button toggles the magnet on or off, using the last-chosen mode. The icon shows which mode is active, and the button looks active while the magnet is on.
  - ⌘ overrides both modes with the High/Low snap.
  - The mode persists in settings.
- **Crosshair.** While any snap is active, the crosshair and its axis labels jump to the snapped point.

### 2.2 "Stay in drawing mode" (replaces ⇧-click-keeps-tool)

⇧ now means angle snap, so it can no longer double as "keep the tool". Instead the left toolbar gets the pencil-with-lock button shown in the reference, placed between the magnet and the lock: **Stay in drawing mode**. While it's on, the tool stays active after each drawing. It persists in settings.

### 2.3 Floating drawing toolbar (replaces the M1 style bar)

The toolbar appears when a drawing is selected, above the drawing (or below it when there's no room), and stays clamped inside the chart. It can be dragged by its grip, and once moved it stays there for the session.

Left to right:

1. **Grip:** drag to move the toolbar.
2. **Templates menu:**
   - "Save Drawing Template As…" asks for a name.
   - "Apply Default Drawing Template".
   - Saved templates for this tool are listed underneath. Each one can be applied, and has a ✕ to delete it.
3. **Line color:** a pencil icon with a color bar under it. It opens the color picker:
   - a grid of 10 columns: a gray row, a saturated row, then 6 tint/shade rows;
   - recent custom colors, plus a "+" to add a custom color (hex input);
   - an **Opacity** slider with a % field.
   - The palette values are XView's own *(my call)*.
4. **Text color:** a "T" with a color bar. It opens the same picker for the drawing's text.
5. **Width:** shows "— 1px". It opens 1, 2, 3 and 4 px options.
6. **Line style:** Line, Dashed line, Dotted line.
7. **Settings:** opens the settings dialog (§2.4).
8. **Alert:** shown disabled, with the tooltip "Alerts (coming soon)". Alerts are milestone 5.
9. **Lock:** locks this drawing only. A locked drawing can be selected but not moved or edited on the chart. Its lock icon is shown closed.
10. **Delete.**
11. **More (⋯):**
    - Clone (offset +10 px)
    - Copy and Paste (⌘C / ⌘V, within the same symbol)
    - Hide
    - Bring to front, Send to back
    - Settings…
    - Delete

**Right-click menu.** Right-clicking a drawing opens the same items as More, plus "Lock" *(my call: a context menu is what "context menu" usually means, and it's cheap once More exists)*.

### 2.4 Drawing settings dialog

Double-clicking a drawing, or the Settings button, opens a modal card. Its header shows the drawing's name with a pencil to rename it (the name defaults to the tool name). It has tabs, and **Template ▾ / Cancel / Ok** at the bottom. Cancel reverts everything, and Ok commits it as one undo step.

The **Trend line** gets the full set of tabs. Other types show only the rows that apply to them (§2.5).

- **Style:**
  - Line: color, style and width.
  - Left end and right end: Normal or Arrow.
  - Extend: Don't extend, Left, Right or Both.
  - Middle point: a checkbox that draws a dot at the midpoint.
  - Price labels: a checkbox that shows each anchor's price on the price axis.
  - INFO, Stats: Hidden, Compact or Values. The stats are price change, %, bar count, time span and angle.
  - Stats position: Left, Center or Right.
  - Always show stats: a checkbox. Unchecked means the stats show only while the drawing is selected or hovered.
- **Text:**
  - Color, size (10–40), bold and italic.
  - A multi-line text box.
  - Alignment: vertical Top, Middle or Bottom, and horizontal Left, Center or Right.
  - The text is drawn next to the line, unrotated, positioned relative to the segment *(my call: rotated text along the line is a later nicety)*.
- **Coordinates:** for each anchor, its price and its **date and time**, both editable.
  - *(My call, deviating from the reference's "bar" number.)* XView stores exact times, and a bar index would change whenever older history loads. Editing the time keeps the no-drift rule.
  - The time is shown in the chart's display timezone.
- **Visibility:** per-timeframe ranges, each with a checkbox, a min and max field, and a two-handle slider:
  - Minutes 1–59
  - Hours 1–24
  - Days 1–366
  - Weeks 1–52
  - Months 1–12

  The drawing is drawn only on chart timeframes inside a checked range. The reference's Seconds, Ticks and Ranges rows are left out, because XView has no such timeframes yet.

### 2.5 Applicability per type

| Row | Trend | Ray | H-line | V-line | Rectangle |
|---|---|---|---|---|---|
| Line color/width/style | ✓ | ✓ | ✓ | ✓ | border |
| Fill color + opacity | – | – | – | – | ✓ |
| Ends, Extend | ✓ | ends | – | – | Extend left/right |
| Middle point | ✓ | ✓ | – | – | ✓ (center) |
| Price labels | ✓ | ✓ | ✓ (default on) | – | ✓ (top/bottom) |
| Stats | ✓ | ✓ | – | – | ✓ (price range, %, bars) |
| Text | ✓ | ✓ | ✓ | ✓ | ✓ |
| Coordinates | 2 points | 2 points | price | time | 2 corners |
| Visibility | ✓ | ✓ | ✓ | ✓ | ✓ |

In the left-toolbar flyout, "Extended line" becomes enabled. It is a trend line created with Extend = Both.

### 2.6 Defaults and templates

- **Last-used style.** A new drawing takes the last-used style of its tool, which is what traders expect. "Apply Default Drawing Template" resets a drawing to XView's built-in default for that tool.
- **Saved templates.** Templates are named, per tool, and stored in `templates.json` in the data directory. That file uses the same atomic write, versioning and corrupt-file handling as the other saved files. Last-used styles are stored there too.

## 3. Non-goals

- Alerts. The alert button is a placeholder.
- Snapping to indicators.
- Rotated text.
- New drawing types (Fibonacci and the rest are milestone 2).
- Copy and paste across symbols.

## 4. Anti-goals

- **Drawing drift.** Snapping and angle constraints only change where an anchor is *placed*. Anchors are still stored as exact time and price, and none of the new options may introduce a screen-space round trip in storage.
- **Losing drawings.** The file format grows, but files written by M1 (version 1) must still load unchanged. New fields default when absent, and they are saved as version 2. A version-2 file opened by M1 being moved aside as corrupt is acceptable.
- **Stutter.** The toolbar, the pickers and the dialog must not re-render the chart shell on every mouse move. Stats and text are drawn only for visible drawings.
- **Looking like TradingView.** The layout and behaviour follow the reference. The icons, palette values, fonts and colors are XView's own.

## 5. Constraints

The constraints from the M1 spec still apply: macOS only, GPL-3.0-or-later, GPUI, logic in GPUI-free crates with tests, and a 60 fps target.

## 6. Left to the builder

- Exact pixel sizes. Match the reference's proportions.
- The picker's exact tint math.
- Arrowhead geometry.
- Stats text format.
- How rectangle Extend draws.
