---
name: easytrace-rs
description: High-speed pure-Rust PCB autorouter (`easytrace`) for JLCEDA Pro (EasyEDA Pro), KiCad, and Specctra DSN/SES workflows. Use when routing multi-layer PCBs, escaping fine-pitch QFN/WLCSP packages, running JLCEDA Pro one-click or headless AI bridge autorouting, or verifying 0-DRC PCB copper completion.
---

# EasyTrace-rs Skill Guide (AI Agents & Automation)

`easytrace` (`EasyTrace-rs`) is a 100% pure-Rust PathFinder negotiated-congestion PCB autorouter built specifically for dense multi-layer boards (0.35 mm QFN, 0.40 mm WLCSP, internal cutouts, and tactile switch NPTH slot bosses) in **JLCEDA Pro (嘉立创EDA专业版)**, **KiCad**, and **Altium**.

## 1. Build Once

```bash
cargo build --release
# Produces ./target/release/easytrace (~2s incremental build, zero C/Python runtime deps)
```

## 2. Choose the Right Subcommand

| Subcommand | Input -> Output | Best For |
|---|---|---|
| `easytrace jlceda <autoroute.json> -o <routes.json>` | JLCEDA `getAutoRouteJsonFile()` -> `routes.json` (+ optional `--ses`) | **Preferred for JLCEDA Pro** — preserves 100% of `prohibitedRegions` (cutouts, `SW*` NPTH bosses, mic ports) & 1oz/2oz DRC clearances! |
| `easytrace dsn <board.dsn> -o <board.ses>` | Specctra `.dsn` -> `.ses` | KiCad, Altium, or zero-plugin Specctra file exchange |
| `easytrace route <geom.json> <routes.json>` | Native EasyTrace geometry JSON -> `routes.json` | Custom CAD preprocessors |
| `easytrace serve --port 3789` | Local HTTP server (`127.0.0.1:3789`) | Human 1-click JLCEDA Pro extension (`dist/easytrace-jlceda.eext`) |

## 3. Closed-Loop Autonomous AI Routing on JLCEDA Pro

When an AI agent has access to a JLCEDA Pro API Bridge (`--bridge-url`, e.g. `http://127.0.0.1:49620`):

1. **Export, Route Offline, and Push in One Step**:
   ```bash
   python3 scripts/bridge_route.py \
     --bridge-url http://127.0.0.1:49620 \
     --pcb-uuid <PCB_UUID> \
     --mode incremental \
     --skip GND \
     --power VBAT,VDD_3V3 \
     --save-dir ./out_route
   ```
   *(Use `--mode replace` to delete unlocked copper tracks & vias before routing; locked user primitives are NEVER touched!)*

2. **Offline Geometric Pre-Check + SVG Inspection**:
   ```bash
   python3 scripts/drc_check.py --autoroute ./out_route/autoroute.json --routes ./out_route/routes.json
   python3 scripts/render_svg.py --autoroute ./out_route/autoroute.json --routes ./out_route/routes.json -o ./out_route/routes.svg --split-layers
   ```

3. **Native JLCEDA Pro Engine DRC Verification (Critical `--reopen` Tip!)**:
   > **JLCEDA Pro API Quirk**: Vias created via `eda.pcb_PrimitiveVia.create()` do not register net connectivity in JLCEDA's in-memory DRC until the PCB tab is closed and reopened! Always pass `--reopen`:
   ```bash
   python3 scripts/drc_check.py \
     --bridge-url http://127.0.0.1:49620 \
     --pcb-uuid <PCB_UUID> \
     --project-uuid <PROJECT_UUID> \
     --reopen
   ```
   Target sign-off: `nonConnCount == 0` and `sigUnconn == 0` (pass `--pour GND` to `bridge_route.py` — it automatically creates pours on all active copper layers and triggers JLCEDA Pro's internal `MSG_BUS_PCB.messageBus.publish("pcb/pcbRebuildCopperRegion")` + `publish("rebuildCopper")` RPC with **zero manual `Shift+B` hotkey**, then verifies `poured` count so `gndUnconn == 0`).

## 4. Machine-Readable `stdout` Progress Protocol

`easytrace` writes human diagnostics to `stderr` and structured progress lines to `stdout`:

- `PROG <elapsed_sec> <clean_routed_conns> <clean_complete_nets>` — emitted after setup and each PathFinder iteration.
- `RESULT <routed_conns> <complete_nets> <total_conns>` — emitted once at completion before exiting `0`.
