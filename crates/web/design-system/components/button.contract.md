# COMPONENT: Button

## Variants
- `primary` — `color-accent` fill, `fg-on-accent` text. One per view/section max.
- `secondary` — transparent fill, `border-default` outline, `fg-ink` text.
- `ghost` — no fill, no border, `fg-body` text; background appears only on hover/active.
- `destructive` — `danger` outline + `danger` text at rest; `danger` fill + `fg-on-accent` text on hover/active (destructive intent should require a deliberate hover, not scream at rest).

## Sizes
- `sm` — 28px height, `text-xs`, `space-1`/`space-3` padding — dense toolbar contexts.
- `md` (default) — 36px height, `text-sm`, `space-2`/`space-4` padding.
- `lg` — 44px height, `text-base`, `space-3`/`space-6` padding — primary page-level actions only.
- `icon-only` — square, matches `sm`/`md`/`lg` height, icon centered, requires `aria-label`.

## States
| State | Treatment |
|---|---|
| default | per variant above |
| hover | background/border shifts one step (`accent-hover`, `border-strong`, etc.); `duration-fast` + `ease-enter` |
| active | shifts a second step (`accent-active`) |
| focus-visible | 2px `color-accent` outline, 2px offset — always visible, never suppressed |
| disabled | `opacity: 0.4`, `cursor: not-allowed`, no hover/active response, still keyboard-focusable to announce state via `aria-disabled` (not `disabled` attribute) when the reason should be discoverable |
| loading | icon slot replaced by `spinner-inline` (see `loading-states.contract.md`), label stays visible unless width-constrained, button non-interactive (`aria-busy="true"`) |

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
- Hover/active: `duration-fast` (150ms), `ease-enter`.
- Loading spinner: `spinner-rotate` per `loading-states.contract.md`.

## Slots
- `leading-icon`, `label`, `trailing-icon` (trailing reserved for a directional affordance,
  e.g. chevron on a menu-trigger button — never a naked decorative arrow).

## Do / Don't
- Do: one `primary` button per view or per modal footer; every other action is `secondary` or
  `ghost`.
- Do: keep destructive actions behind a confirm step (modal) — the button itself commits, but
  the click before it is a real confirmation, not a color warning alone.
- Don't: use `radius-avatar` (pill shape) on buttons — `radius-surface` (0) is the system
  default; a pill button would be the one component breaking the sharp-grid signature for no
  reason.
- Don't: stack two `primary` buttons side by side — dilutes the one-primary-action rule.
