<div align="center">

# EasyTrace-rs (`easytrace`)

**High-Speed Pure-Rust PathFinder Negotiated-Congestion PCB Autorouter**  
*Native Cutout & NPTH-Slot Aware Routing for JLCEDA Pro (EasyEDA Pro), KiCad, and Specctra DSN/SES*

[**English**](README.md) | [**简体中文**](README.zh-CN.md) | [**日本語**](README.ja.md)

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Pure_Rust-2021_Edition-orange.svg)](Cargo.toml)
[![JLCEDA Pro](https://img.shields.io/badge/JLCEDA_Pro-v2.3%2B_%2F_v4.1%2B-00b96b.svg)](extension/easytrace-jlceda-ext)

</div>

---

**EasyTrace-rs** is a standalone, zero-Python-dependency PCB autorouter written in 100% pure Rust (`serde` + `serde_json` only — compiles in seconds). Born from real-world routing of ultra-dense 4-layer and 6-layer wearable PCBs (`0.35 mm` pitch QFN-68, `0.40 mm` pitch WLCSP-9, large central battery cutouts, and tactile switch NPTH bosses), EasyTrace-rs delivers **100% JLCEDA DRC-clean octilinear (`0°/45°/90°`) routing** in seconds to a couple of minutes.

---

## Can JLCEDA Pro (嘉立创EDA专业版) Use a Third-Party External Autorouter *Without* an AI Bridge?

**YES — in TWO different human-friendly ways!** Plus a third headless closed-loop workflow built specifically for AI coding agents:

| Workflow | Needs AI Bridge? | Needs Extension? | Preserves Cutouts & NPTH Bosses? | Target Users |
|---|:---:|:---:|:---:|---|
| **Way 1: 1-Click Local Extension + `easytrace serve`** *(Recommended)* | **No** | Yes (`.eext` included) | **100% Preserved** (`prohibitedRegions` + 1oz/2oz rules) | Human PCB designers in JLCEDA Pro Desktop & Web (`pro.lceda.cn`) |
| **Way 2: Specctra `.dsn` -> `.ses` File Exchange** | **No** | **No** | Standard keepouts (see JLCEDA `.dsn` caveat below) | JLCEDA Pro, **KiCad**, **Altium Designer**, Eagle |
| **Way 3: Headless AI Bridge + Closed-Loop DRC** | Yes | No | **100% Preserved** + live native DRC verification | Claude Code, Antigravity, Cursor, Codex agents |

---

## Quick Installation

```bash
git clone https://github.com/eggfly/EasyTrace-rs.git
cd EasyTrace-rs
cargo build --release

# Binary is ready at:
./target/release/easytrace --help
```

---

## Three Ways to Route

### Way 1: Zero-Bridge One-Click Local Extension for Humans *(Recommended for JLCEDA Pro)*

Instead of fighting `.dsn` export bugs or setting up any AI automation bridge, human designers can route any open PCB in **1 click** using the bundled `.eext` extension:

1. **Start the lightweight local Rust server** in a terminal (uses `std::net::TcpListener`, zero heavy runtime):
   ```bash
   ./target/release/easytrace serve
   # Listening on http://127.0.0.1:3789 (supports both Desktop & Web https://pro.lceda.cn via PNA CORS)
   ```
2. **Install the bundled JLCEDA Pro extension** once:
   - Open JLCEDA Pro (Desktop client or Web `https://pro.lceda.cn`).
   - Go to **Settings -> Extensions -> Install Extension...** (`设置 -> 扩展 -> 安装扩展`) and select [`dist/easytrace-jlceda.eext`](dist/easytrace-jlceda.eext) (rebuild anytime with `python3 extension/easytrace-jlceda-ext/build_eext.py`).
3. **Click `EasyTrace-rs -> Auto Route Current PCB...`** (or **`Quick Auto Route (1-Click)`**) in the top PCB menu bar:
   - The extension calls `await eda.pcb_ManufactureData.getAutoRouteJsonFile()` — capturing **100% of internal board cutouts, `SW*` tactile switch NPTH bosses, MEMS microphone acoustic ports (`prohibitedRegions`), bottom-side `MIRROR_Y` footprints, and 1oz/2oz DRC clearance tables**.
   - Posts the JSON to `http://127.0.0.1:3789/route-jlceda` and writes every routed `pcb_PrimitiveLine` and `pcb_PrimitiveVia` directly onto your PCB canvas!

---

### Way 2: Universal Zero-Plugin File Exchange (`Specctra .dsn -> .ses`)

Works out-of-the-box with **KiCad**, **Altium Designer**, and **JLCEDA Pro** with zero plugins installed:

1. **Export `.dsn`**:
   - **JLCEDA Pro**: `File -> Export -> Auto Route File (DSN)...`
   - **KiCad PCB Editor**: `File -> Export -> Specctra DSN...`
2. **Run `easytrace dsn`**:
   ```bash
   ./target/release/easytrace dsn board.dsn -o board.ses --skip GND --power VBAT,VDD_3V3
   ```
3. **Import `.ses`**:
   - **JLCEDA Pro**: `File -> Import -> Auto Route Session File (SES)...`
   - **KiCad PCB Editor**: `File -> Import -> Specctra Session...`

> [!WARNING]
> **Known JLCEDA Pro v4.1 `.dsn` Exporter Limitation:**  
> JLCEDA Pro's native `.dsn` exporter omits `Slot-to-Track` (`0.30 mm`) keepout margins on internal multi-layer cutouts and footprint NPTH bosses (exporting them as bare unplated padstacks with `0.10 mm` signal clearance instead of `prohibitedRegions`), and flattens 1oz/2oz clearance tables. If your JLCEDA Pro PCB contains internal cutouts, battery windows, or switches with NPTH bosses, **prefer Way 1 (`easytrace serve` + `.eext`) or `easytrace jlceda`**, which reads `getAutoRouteJsonFile()` directly and honors every cutout and slot clearance to the micron!

You can also run the native JLCEDA JSON router directly from CLI (and optionally emit a JLCEDA-aligned `.ses` file at the same time):

```bash
./target/release/easytrace jlceda autoroute.json -o routes.json --ses routes.ses --skip GND
```

---

### Way 3: AI-Agent Native — Headless Bridge + Closed-Loop DRC

EasyTrace-rs ships with project-agnostic scripts ([`scripts/`](scripts)), [`SKILL.md`](SKILL.md), and [`AGENTS.md`](AGENTS.md) so AI agents (**Claude Code**, **Antigravity**, **Cursor**, **Codex**) can autonomously route boards and verify **0 DRC violations**:

```bash
# 1. Export AutoRouteJson via Bridge, route in pure Rust, and push tracks/vias back:
python3 scripts/bridge_route.py \
  --bridge-url http://127.0.0.1:49620 \
  --pcb-uuid <PCB_UUID> \
  --mode incremental \
  --skip GND \
  --power VBAT,VDD_3V3 \
  --save-dir ./run_artifacts

# 2. Offline geometric DRC + multi-layer SVG inspection (zero pip packages needed!):
python3 scripts/drc_check.py --autoroute ./run_artifacts/autoroute.json --routes ./run_artifacts/routes.json
python3 scripts/render_svg.py --autoroute ./run_artifacts/autoroute.json --routes ./run_artifacts/routes.json \
  -o ./run_artifacts/board_routes.svg --split-layers

# 3. Live JLCEDA Pro native DRC check (--reopen ensures newly created vias register net connectivity):
python3 scripts/drc_check.py \
  --bridge-url http://127.0.0.1:49620 \
  --pcb-uuid <PCB_UUID> \
  --project-uuid <PROJECT_UUID> \
  --reopen
```

---

## Engine Architecture Highlights

1. **Zero-Bloat Exact Midpoint Diagonal Maps (`st_ortho` + `st_mid`)**:
   - Traditional grid routers inflate diagonal obstacles by `h * sqrt(2) / 2` (`+0.0355 mm` radius = **`0.071 mm` phantom bloat**), sealing shut `0.35 mm`-pitch QFN corridors and `0.40 mm`-pitch WLCSP-9 diagonal throats (`0.357 mm` copper gap).
   - EasyTrace-rs evaluates static obstacles at exact grid vertices (`st_ortho`) **and** half-shifted cell centers `(x0 + 0.5h, y0 + 0.5h)` (`st_mid`, which simultaneously coincides with both NE/SW and NW/SE square diagonals!), unlocking fine-pitch escapes cleanly.
2. **Split Dynamic Congestion Maps (`dy_ortho` vs `dy_diag`) — +20% Channel Capacity**:
   - Orthogonal `0.127 mm` traces pack on `0.250 mm` centers (5 grid cells at `h = 0.05 mm`, leaving `0.123 mm >= 0.102 mm` copper gap) instead of `0.300 mm` (6 cells), increasing parallel bus density by **20% across every copper layer**, while vias (`vst` / `vdy`) use exact vertex-aligned clearances without diagonal bloat.
3. **Same-Net Fanout Via-Near-Pad Unlock**:
   - Paints full `vcl + vr` (`~0.355 mm`) with `net` so foreign nets maintain full pad-to-via clearance, while painting `0x4000_0000 | net` **only** within `via_hole_d / 2.0 + 0.02 mm` of pads — allowing same-net fanout vias right next to QFN/0402 pad toes while keeping drill holes strictly outside SMD pads (no solder wicking).
4. **Additive VPR PathFinder + Per-Branch Steiner Rip-Up + Monotonic Rescue Pass**:
   - Uses pure additive VPR negotiated congestion `c = base * wall_disc + dir_pen + base * (hist + pres * occ)` (`pres` capped at `28.0`, weighted A* `1.32x`, inner-layer alternating H/V preference), ripping up **only conflicting Steiner branches** while keeping clean branches of multi-pin nets intact.
   - Follows non-destructive single-branch legalization with a **targeted rip-and-reroute rescue pass** (`pres = 200`, `no_hist = true`) that displaces at most 1–3 blocking foreign branches, re-routes them with `hard: true`, and commits strictly when total legal connections increase.
5. **Automatic Power-Net B20 Pad Neckdown & `orig_edges`-Safe Octilinear Smoothing**:
   - Wide power nets (`0.200 mm`) automatically neck down to `0.127 mm` (`5 mil`) within `0.55 mm` of component pads (with optional `--min-width 0.095` pinch squeeze for `3.5 mil` 6-layer boards).
   - Enforces exact drill `hole2Hole >= 0.305 mm` (both diff-net and same-net vias), separate `slot_track` (`0.31 mm`) vs `slot_via` (`0.19 mm`) clearances, clean tiny-pad centroid snapping, and `orig_edges`-verified octilinear pull-tight smoothing.

---

## Real-World Benchmark (Dense 4–6 Layer Circular Watch PCB w/ Central Cutout, 226 Connections)

| Autorouter | Runtime | Routed Completion | JLCEDA Native DRC Violations | Notes |
|---|---:|---:|---:|---|
| **EasyTrace-rs (`easytrace`)** | **12 s – 95 s** | **97.3% – 100%** | **0 violations** | Honors cutouts, `SW*` NPTH bosses, `hole2Hole >= 0.305 mm`, & `0.127 mm` B20 neckdown |
| **Java FreeRouting v2.x** (`.dsn`/`.ses`) | 4 min – 15+ min | Stalls on plateau | Multiple | Shrinks neck traces `< 0.127 mm`; `.dsn` export drops JLCEDA slot keepouts |
| **JLCEDA Pro Built-in JS Router** | 14+ min (blocks UI) | < 15% on dense QFN | 0–5 | Freezes editor UI; struggles on `0.35 mm` QFN + central battery cutout detours |
| **Python Grid Maze Router** | > 6 hours | ~55% | Clearances `< 0.152 mm` | Orders of magnitude slower without negotiated rip-up |

---

## CLI Reference

```text
easytrace dsn <input.dsn> -o <output.ses> [OPTIONS]
easytrace jlceda <autoroute.json> -o <routes.json> [--ses <out.ses>] [OPTIONS]
easytrace route <geom.json> <routes.json> [OPTIONS]
easytrace serve [--port 3789] [--host 127.0.0.1]
```

Key flags:
- `--iters <N>`: Max PathFinder rip-up iterations (default: `45`)
- `--h <MM>`: Routing grid pitch in mm (default: `0.05`)
- `--skip <NETS>`: Comma-separated nets left for copper pour (default: `GND`)
- `--power <NETS>`: Comma-separated `0.200 mm` power nets with automatic `0.127 mm` pad neckdown
- `--ignore-existing` / `--ignore-wiring`: Ignore pre-existing tracks/vias and route the entire board from scratch
- `--min-width <MM>`: Optional pinch-throat squeeze width (e.g. `0.095` for 6-layer `3.5 mil` rules; omitted by default to keep all traces `>= 0.127 mm`)
- `--algo <pf|lee|solo>`: Negotiated PathFinder (`pf`, default), single-pass greedy (`lee`), or independent net feasibility (`solo`)

---

## License

Released under the [MIT License](LICENSE).
