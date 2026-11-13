# Node view profiling

The November 2026 profile found that scene construction alone understated node
view's cost. GTK still rasterized Cairo recordings during camera movement, and
the full-canvas preview mask required offscreen composition. The initial
40-window sample spent about 0.45 ms constructing a scene but 9.9 ms painting it.
User-space `perf` samples put approximately 31% of shell CPU cycles in Cairo and
another 14% in Pixman on the performance cores, including startup and warm-up.

The renderer now clips individual drawing nodes to the visible canvas region,
distributing clips over containers so an unrelated preview hole does not replay
every card. The grid uses GSK's repeat node. Workflow cards use the same retained
local drawing path as window metadata. Card contents are rasterized to textures
at power-of-two resolution tiers, rounded up for the current transform and
output scale. Camera movement within a tier reuses those pixels; content, theme,
number labels and output-scale changes invalidate the appropriate card. Only
one current texture is retained per card. Borders, search highlights, movement,
opacity and native application buffers remain independent of the cached text.
Empty background drawing layers are omitted.

## Comparison

These measurements use release builds, the same compositor binary, a private
1600×1000 Xvfb/niri desktop, five open/close cycles, and five rounds of zoom and
keyboard navigation. The baseline is `338f70f45` with only the new paint/camera
timing instrumentation. The mixed fixture contains six application windows and
40 Constant cards. No CPU sampling is enabled in these timing runs.

Values are milliseconds. Means cover the complete run; p95 covers the most
recent 120 samples, matching the performance endpoint's bounded sample history.

| Fixture and measurement | Before mean | After mean | Before p95 | After p95 |
| --- | ---: | ---: | ---: | ---: |
| 40 windows: transition GTK paint | 5.15 | 2.02 | 8.22 | 4.01 |
| 40 windows: transition frame gap | 15.04 | 9.83 | 24.73 | 19.05 |
| 40 windows: camera GTK paint | 4.35 | 1.78 | 9.58 | 3.56 |
| 40 windows: camera frame gap | 20.03 | 16.13 | 35.19 | 26.15 |
| Mixed: transition scene construction | 1.81 | 0.43 | 7.87 | 1.74 |
| Mixed: transition GTK paint | 11.44 | 1.72 | 21.68 | 3.80 |
| Mixed: transition frame gap | 19.45 | 7.59 | 31.96 | 14.69 |
| Mixed: camera GTK paint | 5.37 | 1.44 | 22.52 | 2.34 |
| Mixed: camera frame gap | 10.45 | 8.28 | 32.11 | 11.89 |

The mixed fixture's measured transition and camera p95 gaps are below the
16.7 ms budget for 60 Hz. The 40-window camera mean approaches that budget,
although its p95 remains above it. These are client timings in a nested desktop
whose compositor uses software rendering, not physical presentation rates or a
claim of parity with native niri. The configured 220 ms animation duration is
unchanged. A host-GPU run must keep the nested window visible; hidden windows
receive throttled frame callbacks and produce invalid timing comparisons.

Raw reports from this investigation are saved locally under
`artifacts/node-profile/`: `final-before-40.json`, `texture-after-40.json`,
`final-before-mixed.json`, and `texture-after-mixed.json`. Artifacts are ignored by
Git. A 100-window startup burst failed in the baseline desktop provider with an
existing registration-token conflict before profiling could start; this report
does not claim results for that workload.

## Reproduction and validation

```sh
python3 scripts/benchmark_views.py --windows 40 --samples 5 --camera \
  --flowwm target/release/flowwm --compositor target/niri/release/niri \
  --output artifacts/view-benchmark/windows.json
python3 scripts/benchmark_views.py --windows 6 --nodes 40 --samples 5 --camera \
  --flowwm target/release/flowwm --compositor target/niri/release/niri \
  --output artifacts/view-benchmark/mixed.json
python3 scripts/benchmark_views.py --windows 6 --samples 2 --verify \
  --flowwm target/release/flowwm --compositor target/niri/release/niri \
  --output artifacts/view-benchmark/verification.json
xvfb-run -a python3 scripts/smoke_rendering.py
```

Use `--flowwm` to select an adjacent saved set of `flowwm`, `flowwm-shell`,
`flowwm-desktop` and `flow-core` binaries for comparison. Add `--profile` for a
shell CPU trace or `--backend wayland` for a visible host-GPU run. See
[development instructions](development.md#view-performance-and-input-checks)
for the timing fields and dependencies.

Graphical regressions compare the new clips against the previous mask through
the actual GSK renderer, allowing at most one channel value of rounding from the
extra offscreen group. They also check native preview transparency, full
selection outlines, foreground gestures, unchanged card reuse during movement,
content refresh, and bounded cache replacement on style changes. The view
benchmark checks zero changed settled chrome pixels after repeated peeks,
inverse zoom restoration, rapid reversals, row restoration, and closing windows
without affecting peers in the same application process.
