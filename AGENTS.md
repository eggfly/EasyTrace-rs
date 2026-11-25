# AGENTS.md — EasyTrace-rs Repository Guide

## Overview

**EasyTrace-rs** (`easytrace`) is a pure-Rust PathFinder negotiated-congestion PCB autorouter with first-class support for **JLCEDA Pro (EasyEDA Pro)** (`getAutoRouteJsonFile()` + 1-click `.eext` extension) and **Specctra DSN/SES** (`KiCad`, `Altium`, `JLCEDA Pro`).

## Repository Layout

```text
src/
  lib.rs        # Library root exporting model, engine, dsn, jlceda, server
  main.rs       # `easytrace` CLI (`dsn`, `jlceda`, `route`, `serve`)
  model.rs      # Geometry primitives (Poly, Seg, Circ), DRC rules, pure-Rust island Union-Find
  engine.rs     # v3.2 PathFinder negotiated-congestion octilinear autorouter
  jlceda.rs     # Pure-Rust JLCEDA Pro `AutoRouteJson` parser + arc discretizer + SES exporter
  dsn.rs        # Pure-Rust Specctra `.dsn` S-expression parser + `.ses` session writer
  server.rs     # Zero-dependency `std::net::TcpListener` HTTP server (`127.0.0.1:3789`)
extension/
  easytrace-jlceda-ext/   # JLCEDA Pro 1-click local extension source + `build_eext.py`
dist/
  easytrace-jlceda.eext   # Ready-to-install JLCEDA Pro extension package
scripts/
  bridge_route.py         # Reconnect-safe headless AI bridge autorouting script
  drc_check.py            # Offline geometric DRC + live JLCEDA native DRC verifier
  render_svg.py           # Zero-dependency multi-layer SVG visualizer
tests/
  router_tests.rs         # End-to-end Rust unit & integration tests
```

## Key Invariants (Do Not Regress!)

1. **Zero Phantom Diagonal Bloat**: Static obstacles use exact vertex `st_ortho` (`+0.004 mm`) + half-shifted cell-center `st_mid` (`(x0 + 0.5h, y0 + 0.5h)`, `+h*sqrt(2)/4`). Never revert to `st_diag` with `+h*sqrt(2)/2` margin, which blocks `0.40 mm`-pitch WLCSP-9 diagonal escapes.
2. **Default `0.127 mm` (`5 mil`) Minimum Width + Opt-In Squeeze (`--min-width 0.095`)**: JLCEDA Pro's default board rules flag `< 0.127 mm` as `Physical Error | Track`. Keep default `widths = [0.127, 0.200]` (`0.200 mm` power necking to `0.127 mm` near pads) unless `--min-width <MM>` is explicitly passed.
3. **Split `dy_ortho` vs `dy_diag`, `orig_edges` Smoothing, & Vertex-Aligned Vias**: Dynamic trace maps split `dy_ortho` (`+0.004 mm`) vs `dy_diag` (`+margin`), `smooth_net` checks `orig_edges` (not just `orig` vertices!) before chamfering 90-degree bends into 45-degree diagonals, and `vst`/`vdy` use `+0.004 mm` without margin bloat, enabling `0.250 mm` (5-cell) parallel `0.127 mm` trace pitch.
4. **Same-Net Fanout Via-Near-Pad Unlock**: `vst` paints full `vcl + vr` with `net` (protecting foreign nets) and paints `0x4000_0000 | net` ONLY with `via_hole_d / 2.0 + 0.02` on `pad`s so same-net vias can sit right next to QFN/0402 pad toes without drill-in-pad wicking.
5. **Exact JLCEDA Bottom-Layer Transform**: In `getAutoRouteJsonFile()`, bottom-mounted components (`layer == 2`) apply `MIRROR_Y` (`ly = -ly`) **before** CCW rotation (`jerboa/worker.js` convention), and single-sided SMD pad layers swap `1 <-> 2`.
6. **Additive VPR PathFinder Cost**: Step cost uses `base_step * wall_disc + sq_pen + dir_pen + base_step * (hist + pres * occ)` with `pres` capped at `28.0` and weighted A* (`1.32` when `!hard`, `1.0` when `hard`).

## Verification Commands

```bash
cargo test
cargo build --release
python3 extension/easytrace-jlceda-ext/build_eext.py
```
