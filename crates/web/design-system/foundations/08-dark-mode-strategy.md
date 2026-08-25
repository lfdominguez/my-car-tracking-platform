# Dark mode strategy

## Principle

Dark is the primary mode for this product — it's the terminal aesthetic's native state, and
it's what the existing `index.html` already declares (`color-scheme: dark`, dark
`theme-color`). Light mode is a genuine, independently-designed second mode, not a filter
applied to dark mode. Nothing here is computed by inverting a hex value; every pairing below was
picked, then contrast-checked, then in three cases (marked) manually re-tuned because the
straightforward pick didn't clear AA.

## Why each pairing is what it is (not an inversion)

**Canvas/surface direction reverses, not just its lightness.** In dark mode, `bg-canvas`
(`#0a0a0a`) is *darker* than `bg-surface` (`#171717`) — surfaces sit "up" from the canvas. In
light mode, `bg-canvas` (`#f4f4f5`) is *darker* (more gray) than `bg-surface` (`#ffffff`) —
surfaces are still "up," but the direction of the RGB delta flipped, because the ceiling in
light mode is white, not black. An inversion script would have picked `bg-canvas: #f5f5f0`-ish
(inverting `#0a0a0a`) and `bg-surface` slightly darker than canvas, which is backwards for how
elevation should read.

**`color-accent` is redefined per mode, not inverted.** Dark mode's accent is near-white
(`#f5f5f4`, the same value as `fg-ink`) because "primary" in an accent-less system means
"maximum contrast against canvas." Light mode's accent is near-black (`#18181b`) for the same
reason — it is the *concept* "primary = highest-contrast neutral" that's consistent across
modes, not a shared hex value. This is a semantic decision (what does "primary" mean here),
not a color-space operation.

**Semantic colors got new lightness values per mode — manual tuning.** This is the one place
a straight swap genuinely fails:
- `warning` dark (`#f1a72b`) is a bright amber that reads clearly on near-black. The same hex on
  white background clears only ~2.3:1 — nowhere near AA. Light mode's `warning` is `#8a5b00`, a
  deep amber-brown picked and contrast-checked specifically for `bg-surface` white (5.9:1).
- `success` and `danger` needed the same treatment for the same reason — dark-mode-bright greens
  and reds are illegible as text on white. Light mode uses deeper `#1a7f37` / `#cf222e`.
- `fg-muted` dark (`#888884`) and light (`#5f5f66`) are *not* the same relative lightness step
  from their respective canvases — light mode's muted needed to be darkened past the "obvious"
  midpoint gray (`#71717a`, which clears only 4.4:1) to `#5f5f66` (5.8:1) to hold AA on the
  lighter canvas. See `07-accessibility.md` for the full before/after.

**Subtle-tint backgrounds (badges, alert fills) use the mode's own text hue, not the other
mode's hue at low alpha.** `success-subtle` in light mode is `rgba(26,127,55,.10)` — derived
from light mode's own `#1a7f37` — not `rgba(52,208,88,.10)` (dark mode's `#34d058` at 10% on
white), which reads as a washed-out mint rather than "success."

## Full pairing table

See `01-color.md` for the complete token-by-token light/dark table and `07-accessibility.md`
for verified contrast ratios on every pairing.

## Verification method

Every text/background pairing above was computed with the WCAG relative-luminance formula
(not eyeballed, not run through a single "check dark, assume light passes" pass) — both modes
independently, both directions (text-on-canvas and text-on-surface). Any pairing landing under
4.5:1 for body-weight text was darkened (light mode) or brightened (dark mode) until it cleared,
then re-checked against its sibling pairing so the two modes still read as the same semantic
color family at a glance, not as two different palettes wearing the same names.

## Common mistakes

- Adding a new semantic color to only one mode "for now." Both light and dark values ship
  together or the token doesn't ship.
- Reusing a dark-mode hex at reduced opacity for a light-mode subtle background — always derive
  from that mode's own text-hue value (see above).
- Testing contrast in dark mode only and assuming light mode inherits the pass — amber/warning
  is the color family most likely to silently fail here.
