# COMPONENT: Alert

Persistent, inline banner — stays on screen until dismissed or the underlying condition
resolves (a car offline, a stale telemetry sync, a form-level validation summary). Distinct
from `toast` (transient, floating, self-dismissing) — see `toast.contract.md`.

## Variants
- `success`, `warning`, `danger`, `info` — `{state}-subtle` background, `{state}`-colored
  left border (2px) and icon, `fg-ink` body text (not colored — only the border/icon carry the
  semantic hue, keeping body copy legible at high contrast regardless of state).

## States
`default`, `dismissible` (close button present), `persistent` (no close button — used when the
underlying condition genuinely can't be dismissed away, e.g. "This car has no active OBD
logger paired").

## Props
```
variant: success | warning | danger | info
title: string?
message: string (required)
dismissible: bool = true
action: { label: string, onClick } ?  (e.g. "Retry sync", "Pair device")
```

## A11y
- `role="alert"` for `danger`/`warning` (interrupts screen reader flow appropriately for
  urgent state), `role="status"` for `success`/`info` (polite, non-interrupting).
- Dismiss button has `aria-label="Dismiss"`.
- Icon is decorative (`aria-hidden="true"`) — the state is conveyed by the real text content,
  the icon reinforces it visually only.

## Motion
- Enter: `duration-medium`, `ease-enter`, height + opacity (the one place a non-transform
  property animates, because collapsing/expanding a banner's height is the correct affordance
  here — mitigate jank by animating `grid-template-rows: 0fr → 1fr` on a wrapper rather than
  raw `height`, which stays GPU-friendlier).
- Exit (dismiss): `duration-fast`, `ease-exit`, reverse.

## Slots
`icon`, `title` (optional), `message`, `action` (optional inline button/link), `dismiss`.

## Do / Don't
- Do: place page-level alerts at the top of the content area, above the first section, full
  content-width.
- Do: use `persistent` (no dismiss) only when dismissing it would hide a genuinely blocking
  condition — don't make transient conditions falsely persistent just to force attention.
- Don't: use `alert` for one-off confirmations ("Trip saved") — that's `toast`'s job. `alert`
  is for conditions, not events.
- Don't: stack more than one alert of the same variant simultaneously — consolidate into one
  message ("2 cars offline") with a link to the detail list.
