# COMPONENT: Button

## Variants
- `primary` — `color-accent` fill (the brand blue) with a soft top-light gradient overlay,
  `fg-on-accent` text, `shadow-accent-glow`. One per view/section max.
- `secondary` — `accent-soft` fill, `border-accent` outline, `color-accent` text.
- `ghost` — no fill, no border, `fg-muted` text; `accent-softer` wash appears on hover/active.
- `destructive` — `danger` outline + `danger` text at rest; `danger` fill + `fg-on-accent` text on hover/active (destructive intent should require a deliberate hover, not scream at rest).

## Sizes
All variants use `radius-control` (11px).
- `sm` — 32px height, `text-sm`, `space-1`/`space-2` padding — dense toolbar contexts.
- `md` (default) — 40px height, ~`text-sm`, `space-2`/`space-4` padding.
- `lg` — 48px height, `text-lg`, `space-3`/`space-6` padding — primary page-level actions only.
- `icon-only` — square, matches `sm`/`md`/`lg` height, icon centered, requires `aria-label`.

## States
| State | Treatment |
|---|---|
| default | per variant above |
| hover | 1px lift + one elevation step (`shadow-1` → `shadow-2`), colour shifts one step (`accent-hover`, `border-strong`), and the `sheen` sweeps once across the face; leading icon nudges 1px |
| active | press back down: `translateY(0) scale(.985)` at 60ms, colour shifts a second step (`accent-active`) |
| focus-visible | the app-wide `--ring` (3px `ring-color`) as a box-shadow, layered over the resting elevation — never an outline jump, never suppressed |
| disabled | `opacity: 0.45`, `cursor: not-allowed`, no lift, no sheen; still keyboard-focusable to announce state via `aria-disabled` (not the `disabled` attribute) when the reason should be discoverable |
| loading | label hidden but its width retained (no layout jump), a centred spinner takes over, hover/sheen suppressed, `aria-busy="true"` |

## Props
```
variant: primary | secondary | ghost | destructive = primary
size: sm | md | lg | icon-only = md
disabled: bool = false
loading: bool = false
icon: IconName? (leading, unless icon-only)
type: button | submit | reset = button
```

## A11y
- Minimum 44×44px hit area regardless of visual size (padding extends the tap target on `sm`).
- `icon-only` requires `aria-label`; never ships icon-only without one.
- `loading` sets `aria-busy="true"`; does not remove the accessible name.
- Never a `div`/`span` with a click handler — always a real `<button>` or, for navigation,
  a real `<a>` styled as a button.

## Motion
- Hover/active: the `press` preset — `duration-fast` (140ms), `ease-standard`; the active
  press is deliberately faster (60ms) than the lift so the control reads as mechanical.
- Hover sheen: the `sheen` preset — one pass, `duration-slow`, never a loop.
- Loading spinner: `spinner-rotate` per `loading-states.contract.md`.
- All of it collapses under `prefers-reduced-motion`; the colour change survives, the
  movement does not.

## Slots
- `leading-icon`, `label`, `trailing-icon` (trailing reserved for a directional affordance,
  e.g. chevron on a menu-trigger button — never a naked decorative arrow).

## Do / Don't
- Do: one `primary` button per view or per modal footer; every other action is `secondary` or
  `ghost`.
- Do: keep destructive actions behind a confirm step (modal) — the button itself commits, but
  the click before it is a real confirmation, not a color warning alone.
- Don't: use `radius-chip` (pill shape) on a general button — that radius belongs to status
  atoms and filter chips. The two documented exceptions are the trips filter chips and the
  traffic "run" control, where the shape is the affordance.
- Don't: put `shadow-accent-glow` on anything but the primary action — two glowing buttons on
  one screen means neither is primary.
- Don't: stack two `primary` buttons side by side — dilutes the one-primary-action rule.
