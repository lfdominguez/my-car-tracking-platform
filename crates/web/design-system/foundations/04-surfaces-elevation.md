# Surfaces, radius & elevation

## Principle

v1 committed to radius 0 and no shadows — a machined, flat grid. v2 replaces both:
the product is a consumer-facing garage cockpit, not a terminal, and the flat grid
read as unfinished rather than deliberate. What replaces it is still disciplined —
one radius scale, one elevation ladder, and no per-component invention.

**Radius.** A real scale, but only three names matter at the call site.
`radius-surface` (16px) is the signature and the default for anything structural —
cards, panels, modals, the KPI strip. `radius-control` (11px) is the slightly tighter
corner for buttons, inputs, and selects, so a control never looks like a card that
lost its content. `radius-chip` (999px) is fully round and belongs to status atoms:
badges, pills, filter chips, segmented controls. `radius-avatar` stays circular.
The raw `xs`–`2xl` steps exist for nested cases (a well inside a card should be
`sm`/`md`, never the same radius as its parent, or the corners look concentric-wrong).

**Elevation.** Depth is now expressed by shadow *and* the surface ladder together,
because light mode never had headroom for a lightness-only ladder — `bg-canvas`
(`#f5f6fa`) sits near the top of the scale, so `bg-surface`/`-raised`/`-overlay` all
resolve to white and a light-mode card had nothing to separate it from the page.

- `shadow-1` — resting controls (buttons, chips).
- `shadow-2` — the default card/panel.
- `shadow-3` — hover on an interactive card; the lift is 2–3px, paired with it.
- `shadow-4` — the auth card, the landing product mock; things that float alone.
- `shadow-overlay` — modal/dropdown/popover/tooltip only. Ring + long blur.
- `shadow-accent-glow` — reserved for the primary action and the gauge ring.
- `shadow-inset-top` — a 1px top highlight, layered onto cards. It's what makes a
  dark surface read as *lit from above* rather than merely lighter.

Dark mode leans on shadow depth plus that top highlight; light mode leans on soft,
wide, very low-alpha shadows. Both are defined per-theme, not tinted at the call site.

The surface ladder still exists and still carries meaning:
`bg-canvas` → `bg-surface` → `bg-surface-raised` → `bg-surface-overlay`, with
`bg-inset` *below* canvas for wells (inputs, metric chips, control tracks).

## Tokens

| Token | Value | Use |
|---|---|---|
| `radius-surface` | 16px | Cards, panels, modals — the signature |
| `radius-control` | 11px | Buttons, inputs, selects, nav items |
| `radius-chip` | 999px | Badges, pills, filter chips, segmented controls |
| `radius-avatar` | 9999px | Avatars, car thumbnails |
| `radius-xs` … `radius-2xl` | 6 / 9 / 12 / 16 / 22 / 28px | Nested surfaces, hero panels |
| `shadow-1` … `shadow-4` | see `tokens.css` | Elevation ladder |
| `shadow-overlay` | see `tokens.css` | Floating layers only |
| `shadow-accent-glow` | see `tokens.css` | Primary action, gauge ring |
| `shadow-inset-top` | 1px top highlight | Layered onto every card |
| `gradient-surface` | see `tokens.css` | Top-light wash on card backgrounds |

## Usage

```css
.card {
  background: var(--gradient-surface), var(--color-bg-surface);
  border: 1px solid var(--color-border-default);
  border-radius: var(--radius-surface);
  box-shadow: var(--shadow-2), var(--shadow-inset-top);
}
a > .card:hover { transform: translateY(-2px); box-shadow: var(--shadow-3), var(--shadow-inset-top); }
.metric-chip { background: var(--color-bg-inset); border-radius: var(--radius-sm); }
.dropdown-panel { background: var(--color-bg-surface-overlay); box-shadow: var(--shadow-overlay); }
```

## Common mistakes

- **Matching a nested well's radius to its parent card.** A 16px well inside a 16px
  card produces visually wrong concentric corners; go one or two steps down.
- **Stacking `shadow-3` on a resting card** to make it "pop." `shadow-3` is the hover
  state; if everything rests at hover elevation, hover has nothing left to say.
- **Lifting a non-interactive card on hover.** The lift is a link affordance. Static
  containers get elevation but no transform.
- **Using `shadow-accent-glow` on a secondary button.** It marks *the* primary action;
  two glowing buttons on one screen means neither is primary.
