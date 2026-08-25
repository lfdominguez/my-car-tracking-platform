# COMPONENT: Toast

Transient, floating notification for one-off events ("Trip saved," "Sync failed — retrying").
Distinct from `alert` (persistent, inline, condition-based) — see `alert.contract.md`.

## Variants
- `success`, `warning`, `danger`, `info` — `bg-surface-overlay` fill, `shadow-overlay`,
  `{state}`-colored leading icon, `fg-ink` message text.
- `auto-dismiss` (default) — clears after 4s (5s for `danger`, since failure messages need
  more read time); pauses the timer on hover/focus.
- `persistent` — requires manual dismiss (used for anything with a required action, e.g. "New
  firmware available — Update now").

## States
`entering`, `visible`, `exiting`, `paused` (auto-dismiss timer paused on hover/focus).

## Props
```
variant: success | warning | danger | info
message: string (required)
action: { label: string, onClick } ?
dismissible: bool = true
duration: auto-dismiss | persistent = auto-dismiss
```

## A11y
- Container: `aria-live="polite"` for all variants, including `danger` — a toast never steals
  focus or interrupts screen reader flow (interrupting flow is `alert`'s job for
  genuinely blocking conditions, not a passing toast).
- Never grabs keyboard focus on appearance — if it has an action button, it's reachable by Tab
  from wherever focus already is, not force-focused.
- Timer pauses on keyboard focus as well as mouse hover, so keyboard users get the same
  extended read time.

## Motion
- Enter: `duration-medium`, `ease-enter`, translateY(8px)→0 + opacity 0→1, stacking from the
  bottom (mobile) or bottom-right (desktop) corner — newest on top of the stack.
- Exit: `duration-fast`, `ease-exit`, reverse; if auto-dismissed, exit is the *only* motion
  (no lingering).
- Multiple toasts stack with `stagger-list-children`-family entry (40ms offset) when several
  arrive in quick succession, capped at 3 visible at once — additional toasts queue.

## Slots
`icon`, `message`, `action` (optional), `dismiss` (optional, per `dismissible`).

## Do / Don't
- Do: cap visible toasts at 3; queue the rest — a wall of stacked toasts on a telemetry-heavy
  product (multiple sync events firing near-simultaneously) is a real risk here specifically.
- Do: give `danger` toasts a real action when one exists ("Retry") rather than a dead-end
  failure notice.
- Don't: use a toast for anything the user must act on before continuing — that's a `modal`,
  not a toast that can auto-dismiss unseen.
- Don't: add a celebration animation (confetti, bounce) to a `success` toast — "Trip saved" is
  calm confirmation, not a celebration, per `09-voice-tone.md`.
