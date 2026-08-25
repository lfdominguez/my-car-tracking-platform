# COMPONENT: Empty state (no-results-state + generic empty)

Covers both flavors this product needs: a genuinely empty collection (no trips logged yet for
this car) and a filtered/searched-to-zero result (no trips match this date range). They share
anatomy; the copy and recovery action differ.

## Variants
- `empty-collection` — nothing exists yet. Copy explains *how* data would appear ("Drive with
  your OBD logger connected and trips appear here automatically"), not just that it's absent.
  Primary action, if any, is the thing that would produce data (e.g. "Pair a device"), not a
  dead-end.
- `no-results` (filtered/searched to zero) — echoes the active query/filter in the heading
  ("No trips found for \"last week\""), offers a clear-filters action and, when reasonable, a
  suggestion (nearest date range that does have data).

## States
`default`. (Empty states are themselves a state of their parent component — they don't have
sub-states of their own beyond entry motion.)

## Props
```
variant: empty-collection | no-results
icon: IconName (Phosphor regular, large size)
heading: string
body: string?
primaryAction: { label: string, onClick } ?
clearFiltersAction: { onClick } ? (no-results only)
```

## A11y
- Rendered as real, readable text — not baked into an image/illustration.
- Heading is a real heading element at the appropriate level for its container (e.g. `<h3>`
  inside a card, `<h2>` for a full-page empty state) so it's navigable by screen-reader users
  jumping by heading.
- Suggested/clear-filter actions are real buttons/links, not styled text.

## Motion
- Entry: `stagger-list-children`-family fade-up (translateY(8px)→0, opacity 0→1),
  `duration-entry` (320ms), `ease-enter` — appears once when the empty state resolves from a
  loading state, not on every re-render.

## Slots
`icon`, `heading`, `body`, `primary-action`, `secondary-action` (clear filters).

## Do / Don't
- Do: differentiate `empty-collection` from `no-results` in copy every time — "No trips yet"
  reads as broken when the real cause is an active filter hiding 40 existing trips.
- Do: use this component for empty table rows too (full table-width row, not a collapsed
  single line) — see `table.contract.md`.
- Don't: ship a vague "No data" with no explanation or action — every empty state in this
  system names what should be there and how to make it appear, per `09-voice-tone.md`.
- Don't: reuse a single generic icon across every empty state in the app (search-empty,
  trips-empty, cars-empty all using the same box glyph) — pick a contextual icon per surface.
