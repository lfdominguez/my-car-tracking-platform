# Surfaces, radius & elevation

## Principle

`data-viz-dense` mandates two things that fight most default component libraries: **radius 0**
and **no shadows for depth**. Both are load-bearing to the aesthetic — a sharp, flat grid is
what makes rows and panels feel machined rather than "app-like." We keep both, with one
narrow, documented exception for floating overlays.

**Radius.** Three tokens, not a scale of six. `radius-surface` (0px) is the default for
everything structural — cards, inputs, buttons, table cells, modals. `radius-chip` (2px) softens
only badges/tags/status pills just enough that dense text doesn't look clipped inside a hard
rectangle. `radius-avatar` (full/9999px) is the one circular exception, reserved for avatars and
car thumbnail photos, because a square user photo reads as a bug, not a style choice.
`radius-signature` is an alias of `radius-surface` — the "signature" of this system *is* zero
radius, so there's no separate brand-fingerprint corner to invent.

**Elevation.** No drop shadows on structural surfaces, in either mode. Depth is expressed by a
four-step surface lightness ladder plus 0.5px hairlines between every row/column
(`border-default`), matching `data-viz-dense`'s own token spec:

`bg-canvas` → `bg-surface` → `bg-surface-raised` → `bg-surface-overlay`

Dark mode has real headroom to climb this ladder (`#0a0a0a` → `#171717` → `#1f1f1f` → `#262626`,
each step a small lightness increase). Light mode does not — `bg-canvas` (`#f4f4f5`) is already
near the top of the neutral scale, so `bg-surface`, `bg-surface-raised`, and `bg-surface-overlay`
all resolve to the same white in light mode; light-mode elevation instead relies on
`border-strong` (a visibly heavier hairline) to separate a raised/overlay surface from its
neighbors.

**The one shadow exception.** A single `shadow-overlay` token exists, used exclusively by
floating layers that sit *on top of* arbitrary page content and cannot rely on a lightness step
alone to read as separated — modal, dropdown, popover, tooltip. It is a near-invisible ring +
long-blur shadow (4–8% alpha in light, a genuine dark shadow at low alpha in dark — never a
glow). It never appears on a card, panel, or table row.

## Tokens

| Token | Value | Use |
|---|---|---|
| `radius-surface` | 0px | Cards, inputs, buttons, modals, table cells |
| `radius-chip` | 2px | Badges, tags, status pills |
| `radius-avatar` | 9999px | Avatars, car thumbnails |
| `bg-canvas` → `bg-surface-overlay` | see `01-color.md` | 4-step elevation ladder |
| `shadow-overlay` | see `01-color.md` | Modal/dropdown/popover/tooltip only |

## Usage

```css
.card { border-radius: var(--radius-surface); background: var(--color-bg-surface); border: 0.5px solid var(--color-border-default); }
.badge { border-radius: var(--radius-chip); }
.avatar { border-radius: var(--radius-avatar); }
.dropdown-panel { background: var(--color-bg-surface-overlay); box-shadow: var(--shadow-overlay); border-radius: var(--radius-surface); }
```

## Common mistakes

- Adding `box-shadow` to a card "for polish." The polish here comes from the hairline grid and
  the lightness ladder — a shadow on a card directly contradicts the style's own philosophy and
  will look like a different, un-committed design system leaking in.
- Using `shadow-overlay` on a structural surface (card, panel) because light mode's ladder looks
  "flat" — that flatness is correct for light mode; `border-strong` is the fix, not a shadow.
- Introducing a fourth radius value ("just this one card needs 8px") — collapse it to
  `radius-surface` (0) or, if it's genuinely a status chip, `radius-chip`.
