# Issue #420 — Settings modal has inconsistent padding

## Overview

### Problem statement

The settings overlay in the wisp crate is drawn by `ModalFrame`
(`crates/wisp/src/surfaces/modal/frame.rs`). The frame gives its content
`Padding::proportional(1)` — 2 columns left/right, 1 row top/bottom — but the
header (title) and footer (key hints) sit directly on the modal's top and
bottom edge rows:

- header/footer: 2 columns horizontal padding, **0 rows vertical padding**
- content: 2 columns horizontal padding, **1 row vertical padding**

The result is a modal whose title and key hints hug the frame's edges while the
content floats inside it.

### Success criteria / acceptance conditions

- The settings modal's header and footer are inset from the modal's edges by
  the same vertical padding the content gets (`MODAL_VERTICAL_PADDING`).
- The content's own padding is unchanged.
- All modal frames share the fix (settings overlay, elicitation request
  modals, review shortcut help), since they render through `ModalFrame`.
- Callers that size a modal from content rows account for the new chrome
  (`MODAL_VERTICAL_CHROME`).
- Very short terminals degrade gracefully: a modal too short to keep a content
  row past the inset drops the inset instead of hiding all content.
- `just lint`, `just fmt-check`, and `just test` stay green.

---

## Technical Approach

`ModalFrame` keeps a single `Block` for the chrome (titles, padding,
background). The block is rendered on the modal rect inset by one row top and
bottom (`chrome_area`), so ratatui's `title_top`/`title_bottom` land one row
inside the modal. `inner()` applies the same inset so content lines up with
what is rendered. The modal background is painted over the full rect because
the inset rows are no longer covered by the block.

The block's padding is spelled out with `Padding::new` from the existing
`MODAL_HORIZONTAL_PADDING` / `MODAL_VERTICAL_PADDING` constants rather than
`Padding::proportional(1)`, so "header/footer padding matches content padding"
is expressed in code instead of relying on `proportional`'s hidden 2x
horizontal scaling.

### Steps

1. `frame.rs`: add `HEADER_FOOTER_VERTICAL_PADDING` (= `MODAL_VERTICAL_PADDING`)
   and `MODAL_VERTICAL_CHROME` (header + footer + their padding + the content's
   padding); inset the block area in `inner()`/render; paint the background
   over the full modal rect; make block padding derive from the constants.
2. `surfaces/modal/mod.rs`: height for content-driven modals becomes
   `body_rows + MODAL_VERTICAL_CHROME`.
3. `screens/review.rs`: shortcut-help modal height becomes
   `rows + MODAL_VERTICAL_CHROME`.

### Test plan

- New integration test `settings_modal_header_and_footer_match_the_content_padding`:
  renders the settings overlay and asserts the header sits one row inside the
  modal's top edge, the footer one row inside the bottom edge, with the
  content's padding row between header and first option (fails before the fix).
- Update `settings_overlay_uses_borderless_modal_chrome_and_padded_highlights`
  to the new header/footer rows.
- Short-height rendering (`settings_overlay_renders_at_short_height`) keeps
  passing via the graceful-degradation path.
- Mouse-activation tests derive click rows from rendered text instead of
  hardcoded row numbers.
