#!/usr/bin/env python3
"""Project-agnostic JLCEDA Pro (EasyEDA Pro) AI Bridge Autorouting Pipeline for EasyTrace-rs.

Exports `getAutoRouteJsonFile()` (preserving 100% of cutouts, NPTH slot bosses, and 1oz/2oz DRC
clearances), runs the native Rust `easytrace jlceda` binary offline, pushes routed tracks and vias
back into JLCEDA Pro using reconnect-safe async polling, and optionally pours GND planes.

Examples:
  # Incremental route on currently open PCB tab:
  python3 scripts/bridge_route.py --bridge-url http://127.0.0.1:49620

  # Rip-up unlocked tracks/vias & route a specific PCB document UUID:
  python3 scripts/bridge_route.py --bridge-url http://127.0.0.1:49620 --pcb-uuid <PCB_UUID> \\
      --mode replace --skip GND --power VBAT,VDD_3V3 --save-dir ./run_out
"""
import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.request
import uuid

MM_TO_MIL = 1.0 / 0.0254


class BridgeClient:
    def __init__(self, base_url: str, project_uuid: str | None = None):
        self.base_url = base_url.rstrip("/")
        self.project_uuid = project_uuid

    def _post(self, path: str, payload: dict, timeout: float = 30.0) -> dict:
        req = urllib.request.Request(
            self.base_url + path,
            data=json.dumps(payload).encode("utf-8"),
            headers={"Content-Type": "application/json"},
        )
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return json.loads(resp.read().decode("utf-8"))

    def exec_sync(self, code: str, timeout: float = 25.0) -> str:
        res = self._post("/execute", {"code": code}, timeout=timeout + 5.0)
        if "error" in res and res["error"]:
            raise RuntimeError(str(res["error"]))
        val = res.get("result")
        return val if isinstance(val, str) else json.dumps(val)

    def select_project_window(self) -> None:
        if not self.project_uuid:
            return
        probe = 'const p=await eda.dmt_Project.getCurrentProjectInfo(); return p?p.uuid:"";'
        try:
            if self.exec_sync(probe, 10.0).strip('"') == self.project_uuid:
                return
        except Exception:
            pass
        try:
            with urllib.request.urlopen(self.base_url + "/eda-windows", timeout=10.0) as r:
                windows = json.loads(r.read().decode("utf-8")).get("windows", [])
        except Exception:
            return
        for w in windows:
            if not w.get("connected"):
                continue
            try:
                self._post("/eda-windows/select", {"windowId": w["windowId"]}, timeout=10.0)
                if self.exec_sync(probe, 10.0).strip('"') == self.project_uuid:
                    return
            except Exception:
                continue
        raise RuntimeError(f"No connected JLCEDA Pro window matches project UUID {self.project_uuid}")

    def run_async(self, js_body: str, tag: str = "__et_job", tmax: float = 900.0) -> str:
        """Reconnect-safe background IIFE execution with chunked string retrieval."""
        self.select_project_window()
        nonce = uuid.uuid4().hex[:10]
        guard = ""
        if self.project_uuid:
            guard = (
                f'const __p=await eda.dmt_Project.getCurrentProjectInfo(); '
                f'if(!__p||__p.uuid!=="{self.project_uuid}") '
                f'throw new Error("Project UUID mismatch: "+(__p&&__p.uuid));\n'
            )
        launcher = (
            f'window.{tag}_st="running {nonce}"; window.{tag}_res=""; '
            f'(async()=>{{ {guard}{js_body} }})()'
            f'.then(r=>{{ window.{tag}_res=(typeof r==="string"?r:JSON.stringify(r)); window.{tag}_st="done {nonce}"; }})'
            f'.catch(e=>{{ window.{tag}_st="ERR {nonce} "+((e&&e.stack)||e); }}); '
            f'return "{nonce}";'
        )
        for _ in range(5):
            try:
                self.exec_sync(launcher, 20.0)
                break
            except Exception:
                try:
                    st = self.exec_sync(f"return String(window.{tag}_st||'');", 15.0)
                    if nonce in st:
                        break
                except Exception:
                    pass
                time.sleep(2.0)
        else:
            raise RuntimeError("Failed to launch async job on JLCEDA bridge")

        t0 = time.time()
        status = ""
        while time.time() - t0 < tmax:
            time.sleep(1.5)
            try:
                status = self.exec_sync(f"return String(window.{tag}_st||'');", 20.0)
            except Exception:
                self.select_project_window()
                continue
            if nonce not in status:
                continue
            if status.startswith("done") or status.startswith("ERR"):
                break

        if status.startswith("ERR"):
            raise RuntimeError(status)
        if not status.startswith("done"):
            raise RuntimeError(f"Async bridge job timed out after {tmax}s (last={status[:160]})")

        total_len = int(self.exec_sync(f"return String((window.{tag}_res||'').length);", 20.0))
        chunks = []
        chunk_sz = 1_500_000
        for off in range(0, total_len, chunk_sz):
            chunks.append(
                self.exec_sync(f"return window.{tag}_res.slice({off},{off + chunk_sz});", 25.0)
            )
        return "".join(chunks)


def find_easytrace_bin(explicit: str | None) -> str:
    if explicit and os.path.isfile(explicit):
        return explicit
    repo_root = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
    for rel in ("target/release/easytrace", "target/debug/easytrace"):
        cand = os.path.join(repo_root, rel)
        if os.path.isfile(cand):
            return cand
    which = shutil.which("easytrace")
    if which:
        return which
    raise FileNotFoundError(
        "Could not find `easytrace` binary. Run `cargo build --release` first or pass `--easytrace-bin`."
    )


def main() -> None:
    ap = argparse.ArgumentParser(description="Bridge-driven PCB autorouting with EasyTrace-rs")
    ap.add_argument(
        "--bridge-url",
        default=os.environ.get("JLCEDA_BRIDGE_URL", "http://127.0.0.1:49620"),
        help="JLCEDA Pro API gateway URL (default: http://127.0.0.1:49620)",
    )
    ap.add_argument("--pcb-uuid", default=None, help="Target PCB document UUID (opens tab if set)")
    ap.add_argument("--project-uuid", default=None, help="Optional project UUID guard")
    ap.add_argument(
        "--mode",
        choices=["incremental", "replace"],
        default="incremental",
        help="`incremental` keeps existing routes; `replace` clears unlocked tracks & vias first",
    )
    ap.add_argument("--skip", default="GND", help="Comma-separated nets to skip (default: GND)")
    ap.add_argument("--power", default="", help="Comma-separated wide power nets (e.g. VBAT,VDD_3V3)")
    ap.add_argument("--iters", type=int, default=45, help="Max PathFinder iterations (default: 45)")
    ap.add_argument("--h", type=float, default=0.05, help="Routing grid step in mm (default: 0.05)")
    ap.add_argument("--min-width", type=float, default=None, help="Optional pinch squeeze width in mm (e.g. 0.095)")
    ap.add_argument("--no-smooth", action="store_true", help="Disable octilinear smoothing")
    ap.add_argument("--pour", default=None, help="Optional net name (e.g. GND) to pour on all layers after routing")
    ap.add_argument("--save-dir", default=None, help="Directory to keep autoroute.json and routes.json")
    ap.add_argument("--easytrace-bin", default=None, help="Path to easytrace binary")
    ap.add_argument("--dry-run", action="store_true", help="Export and route locally without writing to PCB")
    args = ap.parse_args()

    et_bin = find_easytrace_bin(args.easytrace_bin)
    client = BridgeClient(args.bridge_url, args.project_uuid)

    open_snip = ""
    if args.pcb_uuid:
        open_snip = (
            f'for(let __t=0;__t<6;__t++){{'
            f'await eda.dmt_EditorControl.openDocument("{args.pcb_uuid}");'
            f'await new Promise(r=>setTimeout(r,1000));'
            f'const __ci=await eda.dmt_SelectControl.getCurrentDocumentInfo();'
            f'if(__ci&&__ci.uuid==="{args.pcb_uuid}") break;'
            f'}}\n'
            f'const __ci2=await eda.dmt_SelectControl.getCurrentDocumentInfo();'
            f'if(!__ci2||__ci2.uuid!=="{args.pcb_uuid}") throw new Error("wrong active doc: "+(__ci2&&__ci2.uuid));\n'
        )

    if args.mode == "replace" and not args.dry_run:
        clear_js = open_snip + """
const copper = new Set([1, 2]); for (let i = 15; i <= 46; i++) copper.add(i);
const lIds = (await eda.pcb_PrimitiveLine.getAll() || [])
  .filter(l => copper.has(l.getState_Layer()) && !l.getState_PrimitiveLock())
  .map(l => l.getState_PrimitiveId());
if (lIds.length) await eda.pcb_PrimitiveLine.delete(lIds);
const vIds = (await eda.pcb_PrimitiveVia.getAll() || [])
  .filter(v => !v.getState_PrimitiveLock())
  .map(v => v.getState_PrimitiveId());
if (vIds.length) await eda.pcb_PrimitiveVia.delete(vIds);
await eda.pcb_Document.save();
return JSON.stringify({deletedLines: lIds.length, deletedVias: vIds.length});
"""
        print("[bridge] Clearing unlocked copper:", client.run_async(clear_js, "__et_clr", 300.0))

    export_js = open_snip + """
const f = await eda.pcb_ManufactureData.getAutoRouteJsonFile();
if (!f) throw new Error("getAutoRouteJsonFile() returned null — ensure a PCB document is active");
return typeof f === "string" ? f : await f.text();
"""
    print("[bridge] Exporting native AutoRouteJson from JLCEDA Pro...")
    raw_ar = client.run_async(export_js, "__et_exp", 300.0)

    work_dir = args.save_dir or tempfile.mkdtemp(prefix="easytrace_")
    os.makedirs(work_dir, exist_ok=True)
    ar_path = os.path.join(work_dir, "autoroute.json")
    routes_path = os.path.join(work_dir, "routes.json")
    with open(ar_path, "w", encoding="utf-8") as f:
        f.write(raw_ar)

    cmd = [
        et_bin,
        "jlceda",
        ar_path,
        "-o",
        routes_path,
        "--iters",
        str(args.iters),
        "--h",
        str(args.h),
        "--skip",
        args.skip,
    ]
    if args.power:
        cmd += ["--power", args.power]
    if args.mode == "replace":
        cmd.append("--ignore-existing")
    if args.min_width is not None:
        cmd += ["--min-width", str(args.min_width)]
    if args.no_smooth:
        cmd.append("--no-smooth")

    print("[easytrace] Running:", " ".join(cmd))
    proc = subprocess.run(cmd, check=False)
    if proc.returncode != 0:
        sys.exit(proc.returncode)

    routes = json.loads(open(routes_path, encoding="utf-8").read())
    print(
        f"[easytrace] Generated {len(routes['lines'])} segments, {len(routes['vias'])} vias; "
        f"incomplete nets: {list(routes.get('fail', {}).keys())}"
    )

    if args.dry_run:
        print(f"[dry-run] Saved artifacts in {work_dir}")
        return

    ar_obj = json.loads(raw_ar)
    via_sizes = ar_obj.get("rules", {}).get("viaSizes", {})
    default_hole_mm = next(iter(via_sizes.values()), [0.50, 0.30])[1] if via_sizes else 0.30

    js_lines = [open_snip + "let n=0, v=0;"]
    for net, layer, x1, y1, x2, y2, w in routes["lines"]:
        js_lines.append(
            f'if(await eda.pcb_PrimitiveLine.create({json.dumps(net)},{int(layer)},'
            f'{x1 * MM_TO_MIL:.3f},{y1 * MM_TO_MIL:.3f},{x2 * MM_TO_MIL:.3f},{y2 * MM_TO_MIL:.3f},'
            f'{w * MM_TO_MIL:.3f},false)) n++;'
        )
    for net, x, y, d in routes["vias"]:
        hole_mm = 0.20 if d <= 0.451 else default_hole_mm
        js_lines.append(
            f'if(await eda.pcb_PrimitiveVia.create({json.dumps(net)},'
            f'{x * MM_TO_MIL:.3f},{y * MM_TO_MIL:.3f},{hole_mm * MM_TO_MIL:.2f},{d * MM_TO_MIL:.3f})) v++;'
        )
    js_lines.append("await eda.pcb_Document.save(); return JSON.stringify({lines:n, vias:v});")
    print("[bridge] Pushing routes to JLCEDA Pro...")
    push_res = client.run_async("\n".join(js_lines), "__et_push", 1800.0)
    print("[bridge] Push complete:", push_res)

    if args.pour:
        active_layers = ar_obj.get("layers", {}).get("route", [1, 2])
        pour_js = open_snip + f"""
const ol=(await eda.pcb_PrimitivePolyline.getAll()).filter(l=>l.getState_Layer()===11).map(l=>l.getState_Polygon()).filter(g=>g&&g.polygon&&g.polygon[0]!=="CIRCLE");
const src=ol.sort((a,b)=>JSON.stringify(b).length-JSON.stringify(a).length)[0].polygon;
const have=new Set((await eda.pcb_PrimitivePour.getAll()).map(p=>p.getState_PourName&&p.getState_PourName()));
const res={{}};
for (const L of {json.dumps(active_layers)}) {{
  const name = {json.dumps(args.pour)} + "_L" + L;
  if (have.has(name)) {{ res[name] = "exists"; continue; }}
  const p = await eda.pcb_PrimitivePour.create({json.dumps(args.pour)}, L, eda.pcb_MathPolygon.createPolygon(src), "solid", false, name, 1, 10, false);
  res[name] = !!p;
}}
// Zero-hotkey programmatic copper pour rebuild via PCB iframe MSG_BUS_PCB
const __ci = await eda.dmt_SelectControl.getCurrentDocumentInfo();
const __pi = await eda.dmt_Project.getCurrentProjectInfo();
const __fid = (__ci && __pi) ? ("frame_" + __ci.uuid + "@" + __pi.uuid) : null;
const __fr = __fid ? document.getElementById(__fid) : Array.from(document.querySelectorAll("iframe[id^='frame_']")).find(x => x.contentWindow && x.contentWindow.MSG_BUS_PCB);
if (__fr && __fr.contentWindow && __fr.contentWindow.MSG_BUS_PCB && __fr.contentWindow.MSG_BUS_PCB.messageBus) {{
  const __mb = __fr.contentWindow.MSG_BUS_PCB.messageBus;
  try {{ await __mb.publish("pcb/pcbRebuildCopperRegion"); }} catch(e) {{}}
  try {{ await __mb.publish("rebuildCopper"); }} catch(e) {{}}
}}
await new Promise(r=>setTimeout(r,8000));
await eda.pcb_Document.save();
let __poured = 0;
try {{ const __pa = await eda.pcb_PrimitivePoured.getAll(); __poured = __pa ? __pa.length : 0; }} catch(e) {{}}
res.poured = __poured;
return JSON.stringify(res);
"""
        print(f"[bridge] Pouring {args.pour}:", client.run_async(pour_js, "__et_pour", 600.0))


if __name__ == "__main__":
    main()
