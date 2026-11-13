# Desktop activity motion

The wisp communicates **activity, not progress**. Its modes are semantic rather than an arbitrary energy percentage:

| State | Motion | Static silhouette |
| --- | --- | --- |
| Connecting | Slow incomplete orbit | Core with an incomplete ring |
| Thinking | Inward motes, 2.6-second breath | Soft, short flame |
| Compacting | Inward motes and gentle compression | Small core between brackets |
| Working / running tool | Outward motes | Taller flame |
| Writing | Continuous, smooth upward flow | Narrow, elongated flame |
| Waiting for you | None | Amber core with an attention dot and a text label |

Below 32 design pixels the shader uses compact artwork with two motes and no decorative floor ring. Larger artwork adds a subdued floor ring, five motes, and gentle contour detail. Both fit within their layout slot with a 2 px halo margin. There is no rapid twinkling, hue cycling, or central sparkle.

Active modes morph in 320 ms while retaining one shader clock. State changes do not trigger flares. A plan entry changing from in-progress to completed gets one faint, finite 400 ms outline cue; loading a plan, cancelling a step, and changing activity do not. Cues suppressed while reduced, inactive, or clipped are discarded rather than replayed on return. The active mark settles out on completion. Tool failures, cancellation, and completion keep their distinct static icons; disappearing activity is not treated as proof of success.

## Interaction and reduced motion

- Hover the status text to see its activity, current plan step, and running tool.
- Click the status or focus it with the keyboard to open live details. Enter/Space opens it; Escape or an outside click dismisses it. This never cancels a turn.
- **Settings → Activity motion → Reduced** is persisted in desktop state. It removes wisp clocks, mode tweening, entry/exit motion, and milestone cues, but keeps the artwork and labels. It is an explicit app preference, not automatic OS preference detection, and does not disable unrelated glass/menu animations.
- Every running tool has persistent “Running” text, not just a decorative mark. Color is supplementary.

Rouge currently has no native accessibility-tree API. Visible labels and keyboard support improve usability, but are **not** a claim of native screen-reader status announcements. That requires toolkit work.

## Preview the actual shader

```sh
cargo run -p aether-desktop --example wisp_preview
```

The gallery shows light/dark backgrounds, 12/16 px compact and 48 px expanded artwork, manual state controls, rapid transitions, reduced motion, static outcomes, and a scrollable indicator load.

Capture deterministic frames without opening a window:

```sh
cargo run -p aether-desktop --example wisp_preview -- --capture target/wisp-preview.png
cargo run -p aether-desktop --example wisp_preview -- --reduced --scale 2 --capture target/wisp-preview-still@2x.png
```

Measure synchronized headless rendering after shader/pipeline warmup:

```sh
cargo run --release -p aether-desktop --example wisp_preview -- --benchmark
```

The report includes mean/p95 wall time, layout/paint deltas, and rendering pass counts. Wall time includes CPU work, GPU completion, and driver overhead; it is **not GPU-only timing** or a portable performance guarantee. The gallery only renders indicators intersecting its viewport. A separate 32-visible-indicator test asserts work budgets without fragile timing thresholds.

## Visibility and verification

The shared component selects a clock-free effect when waiting, reduced motion is enabled, the window loses focus, or its tracked paint region is clipped out. Hidden application views are unmounted and their clocks are disposed. On resuming a suspended mark, time restarts; ordinary active-mode changes retain their phase.

Rouge's paint-region observer currently does not account for the `invisible` style flag or arbitrary opaque-window occlusion. Hide containing views through conditional mounting (`show`/`dynamic`), as the desktop already does, rather than leaving live wisps under an `invisible`/zero-opacity ancestor.

```sh
cargo test -p aether-desktop --features testing --test desktop wisp::
cargo test -p aether-desktop --features testing --test desktop activity::
cargo test -p aether-desktop --features testing --test desktop settings::
```

Coverage checks actual shader compilation/rendering, distinct static silhouettes at both detail levels and pixel densities, moving active states, identical still frames, clipped/hidden/inactive clocks, live preference changes and persistence, stable layout/art across rapid transitions, live keyboard-accessible details, milestone semantics, and bounded rendering work for 32 compact indicators.
