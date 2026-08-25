# COMPONENT: Settings — grouped section

A section of related settings rows under a heading — units preference, notification
thresholds, device pairing. Each row pairs a label + description with a control, using
`form-field`'s `inline` layout internally.

## Anatomy
`section-heading`, `section-description` (optional), `settings-rows` (list), each row:
`row-label`, `row-description`, `row-control`.

## States
`default`, `control-changed` (unsaved — row gets a subtle `border-strong` left edge until
saved), `saving` (`spinner-inline` replaces/joins the control), `saved-indicator` (brief
"Saved" confirmation, `success` colored, auto-fades after ~2s), `error` (inline error under
the row, per `form-field`'s error slot).

## Props
```
title: string
description: string?
rows: { label: string, description: string?, control: ReactNode, key: string }[]
```

## A11y
- Section heading is a real heading element (`<h2>`/`<h3>` depending on nesting depth) so
  settings pages remain navigable by screen-reader heading jump.
- Each row's control is associated with its label via the same `form-field` wiring
  (`for`/`id`, `aria-describedby` to the description) — not a bare label-shaped `<div>` next to
  an unrelated control.
- `saved-indicator` announces via a polite live region scoped to that row only, not the whole
  page — a settings page with 10 rows saving independently shouldn't produce 10 full-page
  announcements competing with each other.

## Motion
- `save-indicator-fade-in-out`: `duration-medium` in, holds ~2s, `duration-medium` fade out,
  `ease-enter`/`ease-exit` respectively.
- `row-flash-on-change`: on successful save, the row's background briefly steps to
  `success-subtle` for `duration-medium` then eases back to `bg-surface` — confirms *which*
  row saved when several are visible at once, without a persistent color change.

## Slots
`section-heading`, `section-description`, `row` (repeated), `row-label`, `row-description`,
`row-control`, `saved-indicator`.

## Do / Don't
- Do: save each row independently as its control changes (no page-level "Save" button for
  simple preference toggles) — matches the minimal-clicks, low-friction voice target; reserve
  an explicit Save action for rows where a batch of related fields must commit together.
- Do: keep `row-description` genuinely optional and terse — most rows (a unit toggle, a
  threshold number) are self-explanatory from the label alone.
- Don't: use `settings-section-grouped` for a single standalone toggle — that's just a
  `form-field`; this component is for 3+ related rows under one heading.
- Don't: block the row's control while `saving` in a way that discards the user's next
  interaction if they change their mind mid-save — queue the latest value, don't drop it.
