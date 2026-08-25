# COMPONENT: Navigation

## Variants
- `sidebar` — desktop/tablet default (≥`breakpoint-md`), fixed-width (240px) left rail,
  `bg-surface`, `border-default` right edge, sticky full-height.
- `topbar` — mobile default (<`breakpoint-md`), 56px fixed top bar with a menu trigger that
  opens the nav as a `sheet` (see `modal-sheet.contract.md`) rather than a second nav pattern.
- `nav-link` — single item within either variant: icon (`Phosphor` regular, `Phosphor-Duotone`
  when active) + label + optional trailing count badge.
- `breadcrumb` — secondary wayfinding for drill-down views (car → trip → segment detail),
  `text-sm`, `fg-muted` separators, current page in `fg-ink` and non-interactive.

## States
| State | Treatment |
|---|---|
| default (link) | `fg-body` text, Regular icon |
| hover | `bg-surface-raised`, `fg-ink` |
| active/current | `fg-ink` text, `Phosphor-Duotone` icon, `border-strong` left-edge indicator (2px, sidebar only) |
| focus-visible | 2px `color-accent` outline |

## Props
```
items: NavItem[] (label, href, icon, activeIcon, badgeCount?)
variant: sidebar | topbar = responsive (sidebar ≥768px, topbar <768px)
```

## A11y
- `<nav aria-label="Primary">` wrapping the item list; current page marked with
  `aria-current="page"` (drives the active-state styling, not a class check alone).
- Mobile menu trigger is a real button with `aria-expanded`/`aria-controls` pointing at the
  sheet it opens.
- Breadcrumb list uses `<ol>` with `aria-label="Breadcrumb"`; the current (last) item is not a
  link.

## Motion
- Active-indicator shift between items: `duration-fast`, `ease-enter`, background/border only.
- Mobile sheet open/close: per `modal-sheet.contract.md`.

## Slots
`brand` (top of sidebar / left of topbar), `primary-items`, `secondary-items` (settings,
account — bottom of sidebar / inside mobile sheet), `mobile-menu-trigger`.

## Do / Don't
- Do: keep the mobile topbar to one row, 56-64px, with the brand mark left, menu trigger right
  — never a two-row mobile nav.
- Do: use `aria-current="page"` as the actual source of truth for the active styling, so
  server-rendered and client-routed states agree without a hydration flash.
- Don't: put more than 5-7 primary items in the sidebar before grouping into a "More"/settings
  section — this product's minimal-clicks goal is undermined by a long, unscannable nav list.
- Don't: hide primary nav items behind a hamburger on desktop — sidebar stays fully expanded
  ≥768px; the hamburger pattern is mobile-only.
