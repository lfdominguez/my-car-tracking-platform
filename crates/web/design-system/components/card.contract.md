# COMPONENT: Card

## Variants
- `flat` — `bg-surface` fill, `border-default` hairline, no shadow. Default for grouped content
  (a metric summary, a trip detail block).
- `interactive` — `flat` + entire card is a real `<button>`/`<a>`; `bg-surface-raised` on hover,
  `border-strong` on hover, `duration-fast`/`ease-enter`. Used for clickable list-style cards
  (a trip card, a car card) — never a `div` with `onClick`.
- `metric` — a specialized flat card whose primary content is one `text-4xl` mono numeral +
  label + optional delta chip; the KPI hero pattern. At most one `metric` card per view uses
  `text-4xl` — additional metrics on the same view use `text-2xl` or smaller so the hierarchy
  stays legible.

There is no `elevated` (shadow) variant — see `foundations/04-surfaces-elevation.md`; elevation
is the `bg-surface` → `bg-surface-raised` step, not a shadow.

## States
`default`, `hover` (interactive only), `active` (interactive only, `duration-instant` press
feedback), `loading` (see `loading-states.contract.md` — `skeleton-loader-card` mirrors this
component's exact anatomy), `empty` (see `empty-state.contract.md`), `error` (inline error
message replacing content, with a retry action when the failure is refetchable).

## Props
```
variant: flat | interactive | metric = flat
loading: bool = false
padding: default (space-card-pad, 16px)
```

## A11y
- `interactive` cards are real `<button>`/`<a>` elements — the whole surface is the hit target,
  not a nested link inside an otherwise-inert div.
- `metric` cards expose the numeral + label as real text (never an image/canvas render of the
  number) so it's selectable and screen-reader legible.

## Motion
- `interactive` hover/active: `duration-fast`/`duration-instant`, `ease-enter`, background step
  only (no scale/translate — the sharp-grid aesthetic doesn't lift cards off the page).
- `metric` value change: `badge-count-bump` on the numeral when its value updates live.

## Slots
`header` (optional label/eyebrow), `content`, `footer` (optional — delta chip, timestamp,
action link).

## Do / Don't
- Do: use `card` only when the content genuinely needs a bounded surface to be scannable as one
  unit — per `VISUAL_DENSITY=5` and the dashboard-hardening guidance, a lot of this product's
  content (table rows, list items) should use hairlines/negative-space grouping instead of
  wrapping every row in its own card.
- Do: keep card internal padding at `space-card-pad` (16px) consistently — never mix 16px cards
  with 24px cards on the same view.
- Don't: ship three identical-anatomy cards side by side as a layout default (the classic
  3-equal-card pattern) — if three metric cards genuinely belong together, vary their size to
  reflect actual hierarchy (one dominant KPI + two supporting), not three equal boxes.
- Don't: add a drop shadow "just for this one card" — see `04-surfaces-elevation.md`.
