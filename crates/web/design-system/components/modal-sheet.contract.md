# COMPONENT: Modal / Sheet

One contract, two presentations: `modal` (centered dialog, desktop/tablet default) and `sheet`
(bottom-anchored, mobile default). Both share anatomy, states, and a11y; only entry motion and
positioning differ.

## Variants
- `modal` — centered, `max-width` capped at `560px` (`720px` for wider content like a
  `chart-bar-grouped` detail view), `radius-surface` (0), `shadow-overlay`.
- `sheet` — full-width, anchored to the bottom of the viewport, `radius-surface` (0 — no
  rounded top corners; the sharp-grid signature holds even here), drag-handle affordance at
  top, `shadow-overlay`.
- Breakpoint switch: `sheet` below `breakpoint-md` (768px), `modal` at/above — this is the
  same component swapping presentation, not two components maintained separately.

## States
| State | Treatment |
|---|---|
| closed | not rendered (or `display: none` + `inert`) |
| opening | entry motion (see below) |
| open | `overlay-scrim` behind, focus trapped inside, background `inert` |
| closing | reverse of opening |

## Props
```
open: bool
title: string (required — every modal/sheet has a heading, even a short confirm dialog)
onClose: () => void
size: default | wide = default
dismissible: bool = true (Escape key + scrim click close it; false only for a blocking flow like a required first-run setup)
```

## A11y
- `role="dialog"` + `aria-modal="true"` + `aria-labelledby` pointing at the title.
- Focus moves to the dialog on open (first focusable element, or the dialog container itself
  if it starts with static content), returns to the trigger element on close.
- Focus trapped inside while open — Tab cycles within, never escapes to background content.
- Escape key closes when `dismissible`; scrim click closes when `dismissible`.
- Background content gets `inert` (or `aria-hidden="true"` on the app root) while open.

## Motion
- `modal` enter: `duration-medium` (250ms), `ease-enter`, opacity 0→1 + scale 0.98→1 (transform
  only, centered origin).
- `sheet` enter: `duration-medium`, `ease-enter`, translateY(100%)→0.
- Both exit: `duration-fast` (150ms), `ease-exit`, reverse of entry.
- Scrim: opacity fade, `duration-fast`, synced with content motion, not before it.

## Slots
`title`, `close-button` (top-right, icon-only, always present unless `dismissible: false`),
`body`, `footer` (action buttons — one `primary` max, per `button.contract.md`).

## Do / Don't
- Do: match the sheet's drag-handle affordance visually to a small horizontal hairline bar,
  not a decorative pill — consistent with the system's radius-0 discipline elsewhere.
- Do: keep modal body content scrollable independently of the page when it overflows
  (`max-height` capped, internal scroll) — the title/footer stay pinned.
- Don't: nest a modal inside a modal — route to a second step within the same dialog instead.
- Don't: use `backdrop-filter` blur on the scrim behind a modal that itself contains scrolling
  content elsewhere on the page — this is a fixed/sticky-only pattern per the arsenal
  guidelines; the modal itself is fine since it's fixed-positioned, but never blur a
  still-scrolling background panel simultaneously.
