# COMPONENT: Loading states (spinner-inline, skeleton-loader-card, loading-page-shell)

Three prioritized loading primitives, contracted together because they form one escalation
ladder by loading scope — inline action, single card/panel, full page — and should never be
mixed arbitrarily (e.g. a full-page skeleton for a single button's async action).

## spinner-inline
**Use for**: sub-500ms or indeterminate inline waits inside a button, table cell, or list row.
- Anatomy: circular track (`border-default`) + rotating arc (`color-accent` or contextual
  state color) + optional label.
- Motion: continuous rotation, `1000ms` linear loop (the one place a linear ease is correct —
  a spinner has no directional "settle" to ease toward).
- A11y: `aria-label="Loading"` when standalone; when inside a labeled button, the button's own
  `aria-busy="true"` covers it and the spinner itself is `aria-hidden="true"`.
- Reduced motion: replace rotation with a static "loading" glyph or keep a slower rotation
  (never fully static with no indication something is happening).

## skeleton-loader-card
**Use for**: a single card/panel/table-row's content while its data loads — mirrors the
*exact* anatomy of the eventual content (image block, title line, body lines, action block) so
there's no layout jolt on resolve.
- Anatomy: `skeleton-image-block` (only if the real card has one), `skeleton-title-line`,
  `skeleton-body-lines`, `skeleton-action-block`.
- Tokens: `bg-surface` base, `bg-surface-raised`-toned shimmer sweep, `border-default`.
- Motion: `skeleton-shimmer` preset (1500ms linear infinite gradient sweep) — see
  `05-motion.md`.
- A11y: `aria-busy="true"` on the container; `aria-live="polite"` announces "Loading" once
  (not per shimmer cycle).
- Reduced motion: static gray blocks, no shimmer sweep.

## loading-page-shell
**Use for**: initial full-page load (first paint before the WASM app has data), mimicking the
eventual nav + content layout so there's no reflow when real content arrives.
- Anatomy: `nav-skeleton` (sidebar/topbar shape), `content-skeleton-blocks` (using
  `skeleton-loader-card` internally for each panel), shimmer overlay.
- Motion: same `skeleton-shimmer` preset; whole shell fades out (`duration-medium`, `ease-exit`,
  opacity only) once real content is ready — never an abrupt swap.
- A11y: `aria-busy="true"` on the document root/app container while active; the real layout
  underneath must match the skeleton's proportions so no visible "jolt" occurs on removal.

## Do / Don't
- Do: pick the smallest scope that's honest — a single stale metric refetching gets
  `spinner-inline` on just that value, not a full-card skeleton wiping out data the user can
  still see.
- Do: match skeleton anatomy to real anatomy exactly, including a numeric-value skeleton block
  sized/positioned where the mono numeral will render.
- Don't: use a generic circular spinner for a card or full-page load — that's the exact
  anti-pattern this component set exists to replace; skeletons that mirror layout are required
  for anything wider than an inline action.
- Don't: run `skeleton-shimmer` longer than the actual expected wait in a way that reads as
  stuck — if a load routinely exceeds ~8-10s, add a secondary "still loading…" message rather
  than looping the shimmer silently.
