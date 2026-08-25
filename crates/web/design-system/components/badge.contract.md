# COMPONENT: Badge

Small status/label chip — table cell status ("Active", "Offline"), nav item counts, trip tags.
The one component permitted a nonzero, non-full radius (`radius-chip`, 2px) — see
`04-surfaces-elevation.md`.

## Variants
- `neutral` — `border-default` outline, `fg-muted` text, transparent fill. Default for
  non-semantic tags.
- `success` / `warning` / `danger` / `info` — `{state}-subtle` background fill,
  `{state}` text color, no border. Always paired with a short text label, never a bare dot
  (see `07-accessibility.md`).
- `count` — numeric-only badge (nav unread count), `font-family-mono`, tabular, `radius-full`
  — the one place a badge uses `radius-avatar` instead of `radius-chip`, because a numeral
  chip reads better as a circle/pill than a sharp rectangle at that size.

## States
`default`, `updating` (numeral changes trigger `badge-count-bump`).

## Props
```
variant: neutral | success | warning | danger | info | count = neutral
label: string
icon: IconName? (leading, small size only)
```

## A11y
- Never color-only — every semantic badge carries a text label (`"Active"`, not a bare green
  dot).
- `count` badges include an `aria-label` with the full context ("3 unread notifications"), not
  just the bare numeral, when used adjacent to an icon-only trigger.

## Motion
- `badge-count-bump` (360ms, spring-bump easing) on numeral change — see `05-motion.md`.
- No motion on label-only badges appearing/disappearing beyond the parent list's own
  `stagger-list-children` treatment, if any.

## Slots
`icon` (optional, leading), `label`.

## Do / Don't
- Do: reuse the exact `{state}-subtle` / `{state}` token pair from `01-color.md` — don't
  invent a new tint per badge instance.
- Do: keep badge text at `text-2xs`/`text-xs`, uppercase only when it's functioning as an
  eyebrow-style status word ("ACTIVE") with `tracking-wide` applied — not for sentence-case
  labels.
- Don't: stack more than 2-3 badges in a single table cell/row — becomes visual noise in an
  already-dense surface; prefer one primary status badge and push secondary tags to a detail
  view.
