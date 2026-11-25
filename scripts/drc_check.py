#!/usr/bin/env python3
"""Offline & Live-Bridge DRC Verifier for EasyTrace-rs.

Supports TWO modes:
  1. Offline Geometric DRC (zero JLCEDA instance required, zero pip dependencies required!):
     python3 scripts/drc_check.py --autoroute autoroute.json --routes routes.json [--json drc.json]

  2. Live JLCEDA Pro Native Engine DRC via Bridge (with optional tab reopen so vias register):
     python3 scripts/drc_check.py --bridge-url http://127.0.0.1:49620 --pcb-uuid <PCB_UUID> --reopen
"""
import argparse
import collections
import json
import math
import os
import sys
import urllib.request

MM_TO_MIL = 1.0 / 0.0254


def seg_dist(p: tuple[float, float], a: tuple[float, float], b: tuple[float, float]) -> float:
    dx, dy = b[0] - a[0], b[1] - a[1]
    l2 = dx * dx + dy * dy
    t = 0.0 if l2 < 1e-18 else max(0.0, min(1.0, ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2))
    return math.hypot(p[0] - a[0] - t * dx, p[1] - a[1] - t * dy)


def orient(a: tuple[float, float], b: tuple[float, float], c: tuple[float, float]) -> float:
    return (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])


def seg_seg_dist(
    a1: tuple[float, float],
    b1: tuple[float, float],
    a2: tuple[float, float],
    b2: tuple[float, float],
) -> float:
    if (orient(a1, b1, a2) * orient(a1, b1, b2) < 0.0) and (
        orient(a2, b2, a1) * orient(a2, b2, b1) < 0.0
    ):
        return 0.0
    return min(
        seg_dist(a1, a2, b2),
        seg_dist(b1, a2, b2),
        seg_dist(a2, a1, b1),
        seg_dist(b2, a1, b1),
    )


def in_poly(p: tuple[float, float], pts: list[tuple[float, float]]) -> bool:
    c = False
    n = len(pts)
    for i in range(n):
        a, b = pts[i], pts[(i + 1) % n]
        if (a[1] > p[1]) != (b[1] > p[1]) and p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]:
            c = not c
    return c


def discretize_path(raw: list[list[float]]) -> list[tuple[float, float]]:
    pts: list[tuple[float, float]] = []
    for elem in raw:
        if len(elem) < 2:
            continue
        ex, ey = float(elem[0]), float(elem[1])
        if len(elem) >= 3 and pts:
            ang_deg = float(elem[2])
            sx, sy = pts[-1]
            a = math.radians(ang_deg)
            ch = math.hypot(ex - sx, ey - sy)
            if abs(a) < 1e-6 or ch < 1e-7 or abs(math.sin(abs(a) / 2.0)) < 1e-7:
                pts.append((ex, ey))
                continue
            r = ch / (2.0 * math.sin(abs(a) / 2.0))
            mx, my = (sx + ex) / 2.0, (sy + ey) / 2.0
            dd = math.sqrt(max(0.0, r * r - ch * ch / 4.0))
            ux, uy = -(ey - sy) / ch, (ex - sx) / ch
            sgn = 1.0 if (a > 0.0) == (abs(a) < math.pi) else -1.0
            cx, cy = mx + sgn * ux * dd, my + sgn * uy * dd
            a0 = math.atan2(sy - cy, sx - cx)
            steps = max(4, min(36, int(math.ceil(abs(ang_deg) / 15.0))))
            for k in range(1, steps + 1):
                t = a0 + a * k / steps
                pts.append((cx + r * math.cos(t), cy + r * math.sin(t)))
        else:
            pts.append((ex, ey))
    return pts


def run_offline_drc(autoroute_path: str, routes_path: str, min_width: float = 0.127) -> dict:
    ar = json.load(open(autoroute_path, encoding="utf-8"))
    rt = json.load(open(routes_path, encoding="utf-8"))

    sc_map = ar.get("rules", {}).get("safeClearances", {})
    sc = (sc_map.get("copperThickness1oz") or next(iter(sc_map.values()), [{}]))[0] if sc_map else {}
    clr_tt = float(sc.get("trackToTrack", 0.10))
    clr_other = max(float(sc.get("trackToPad", 0.10)), float(sc.get("trackToVia", 0.10)))
    clr_slot_trk = float(sc.get("trackToProhibitedRegion", 0.30))
    clr_slot_via = float(sc.get("viaToProhibitedRegion", 0.152))
    via_sizes = ar.get("rules", {}).get("viaSizes", {})
    via_hole = next(iter(via_sizes.values()), [0.50, 0.30])[1] if via_sizes else 0.30

    outer = discretize_path(ar.get("boardOutline", {}).get("path", []))
    prohibited = [
        (discretize_path(pr.get("path", [])), set(pr.get("layers", [])))
        for pr in ar.get("prohibitedRegions", [])
    ]

    violations = collections.Counter()
    details = []
    eps = 0.003

    new_segs = []
    for net, lay, x1, y1, x2, y2, w in rt.get("lines", []):
        if w < min_width - 1e-4:
            violations["width_under_min"] += 1
        a, b = (float(x1), float(y1)), (float(x2), float(y2))
        hw = float(w) / 2.0
        new_segs.append((net, int(lay), a, b, hw))

        if outer:
            if not (in_poly(a, outer) and in_poly(b, outer)):
                violations["outside_board"] += 1
        for p_pts, p_lays in prohibited:
            if len(p_pts) >= 3 and (not p_lays or int(lay) in p_lays):
                if in_poly(a, p_pts) or in_poly(b, p_pts):
                    violations["inside_prohibited"] += 1
                else:
                    d_pr = min(
                        seg_seg_dist(a, b, p_pts[k], p_pts[(k + 1) % len(p_pts)])
                        for k in range(len(p_pts))
                    ) - hw
                    if d_pr < clr_slot_trk - eps:
                        violations["slot_track_clearance"] += 1
                        details.append({"type": "slot_track", "net": net, "dist": round(d_pr, 4)})

    new_vias = []
    for net, vx, vy, vd in rt.get("vias", []):
        p = (float(vx), float(vy))
        vr = float(vd) / 2.0
        new_vias.append((net, p, vr))
        if outer and not in_poly(p, outer):
            violations["via_outside_board"] += 1
        for p_pts, p_lays in prohibited:
            if len(p_pts) >= 3 and len(p_lays) >= 2:
                if in_poly(p, p_pts):
                    violations["via_inside_prohibited"] += 1
                else:
                    d_pr = min(
                        seg_dist(p, p_pts[k], p_pts[(k + 1) % len(p_pts)])
                        for k in range(len(p_pts))
                    ) - vr
                    if d_pr < clr_slot_via - eps:
                        violations["slot_via_clearance"] += 1

    # Check new-track vs new-track & new-via vs new-via (including hole2Hole >= 0.305mm!)
    for i in range(len(new_segs)):
        n1, l1, a1, b1, hw1 = new_segs[i]
        bx0, bx1 = min(a1[0], b1[0]) - hw1 - 0.3, max(a1[0], b1[0]) + hw1 + 0.3
        by0, by1 = min(a1[1], b1[1]) - hw1 - 0.3, max(a1[1], b1[1]) + hw1 + 0.3
        for j in range(i + 1, len(new_segs)):
            n2, l2, a2, b2, hw2 = new_segs[j]
            if l1 != l2 or (n1 == n2 and n1):
                continue
            if max(a2[0], b2[0]) + hw2 < bx0 or min(a2[0], b2[0]) - hw2 > bx1:
                continue
            if max(a2[1], b2[1]) + hw2 < by0 or min(a2[1], b2[1]) - hw2 > by1:
                continue
            d = seg_seg_dist(a1, b1, a2, b2) - hw1 - hw2
            if d < clr_tt - eps:
                violations["track_to_track"] += 1
                if len(details) < 25:
                    details.append({"type": "track_to_track", "n1": n1, "n2": n2, "layer": l1, "dist": round(d, 4)})

    for i in range(len(new_vias)):
        n1, p1, vr1 = new_vias[i]
        for j in range(i + 1, len(new_vias)):
            n2, p2, vr2 = new_vias[j]
            cdist = math.hypot(p1[0] - p2[0], p1[1] - p2[1])
            if cdist < 1e-4 and n1 == n2:
                continue
            h2h = cdist - via_hole
            if h2h < 0.305 - eps:
                violations["via_hole_to_hole"] += 1
                if len(details) < 25:
                    details.append({"type": "hole2hole", "n1": n1, "n2": n2, "h2h": round(h2h, 4)})
            if n1 != n2 and (cdist - vr1 - vr2) < clr_other - eps:
                violations["via_to_via"] += 1

    length_mm = sum(
        math.hypot(float(l[4]) - float(l[2]), float(l[5]) - float(l[3]))
        for l in rt.get("lines", [])
    )
    return {
        "mode": "offline",
        "segments": len(rt.get("lines", [])),
        "vias": len(rt.get("vias", [])),
        "length_mm": round(length_mm, 2),
        "incomplete_nets": sorted(rt.get("fail", {}).keys()),
        "stats": rt.get("stats", {}),
        "violations": dict(violations),
        "details": details,
    }


def run_bridge_drc(bridge_url: str, pcb_uuid: str | None, project_uuid: str | None, reopen: bool) -> dict:
    base = bridge_url.rstrip("/")
    reopen_snip = ""
    if reopen and pcb_uuid:
        tab_id = f"{pcb_uuid}@{project_uuid}" if project_uuid else pcb_uuid
        reopen_snip = f"""
try {{ await eda.dmt_EditorControl.closeDocument({json.dumps(tab_id)}); }} catch(e) {{}}
await new Promise(r => setTimeout(r, 700));
"""
    open_snip = (
        f'await eda.dmt_EditorControl.openDocument({json.dumps(pcb_uuid)}); await new Promise(r => setTimeout(r, 1200));'
        if pcb_uuid
        else ""
    )
    code = f"""
{reopen_snip}
{open_snip}
const drc = await eda.pcb_Drc.check(false, false, true);
const byType = {{}};
const nonConn = [];
let gndUnconn = 0, sigUnconn = 0;
function walk(n) {{
  if (!n) return;
  if (Array.isArray(n)) {{ for (const x of n) walk(x); return; }}
  if (typeof n === "object") {{
    if (Array.isArray(n.list)) {{ walk(n.list); return; }}
    const t = (n.errorType || "?") + " | " + (n.errorObjType || "?");
    byType[t] = (byType[t] || 0) + 1;
    if (n.errorType === "Connection Error") {{
      if (n.net === "GND") gndUnconn++;
      else sigUnconn++;
    }} else {{
      const ed = (n.explanation && n.explanation.errData) || {{}};
      const pr = (n.explanation && n.explanation.param) || {{}};
      nonConn.push({{
        type: t,
        o1: ed.obj1Suffix || (n.obj1 && n.obj1.suffix),
        o2: ed.obj2Suffix || (n.obj2 && n.obj2.suffix),
        mm: ed.position ? [+(ed.position.x / {MM_TO_MIL / 10.0:.6f}).toFixed(2), +(ed.position.y / {MM_TO_MIL / 10.0:.6f}).toFixed(2)] : null,
        dist: pr.minDistance,
        req: pr.shouldBe
      }});
    }}
  }}
}}
walk(drc);
return JSON.stringify({{ mode: "bridge", byType, gndUnconn, sigUnconn, nonConnCount: nonConn.length, nonConn: nonConn.slice(0, 50) }});
"""
    req = urllib.request.Request(
        base + "/execute",
        data=json.dumps({"code": code}).encode("utf-8"),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=90.0) as resp:
        outer = json.loads(resp.read().decode("utf-8"))
    if outer.get("error"):
        raise RuntimeError(str(outer["error"]))
    return json.loads(outer["result"])


def main() -> None:
    ap = argparse.ArgumentParser(description="Offline & JLCEDA Bridge DRC Checker for EasyTrace-rs")
    ap.add_argument("--autoroute", default=None, help="Path to JLCEDA AutoRouteJson file (offline mode)")
    ap.add_argument("--routes", default=None, help="Path to EasyTrace routes.json file (offline mode)")
    ap.add_argument("--min-width", type=float, default=0.127, help="Minimum allowed trace width in mm (default: 0.127)")
    ap.add_argument(
        "--bridge-url",
        default=None,
        help="JLCEDA Pro API gateway URL for live native DRC (e.g. http://127.0.0.1:49620)",
    )
    ap.add_argument("--pcb-uuid", default=None, help="PCB document UUID for live native DRC")
    ap.add_argument("--project-uuid", default=None, help="Optional project UUID for --reopen tabId")
    ap.add_argument("--reopen", action="store_true", help="Close & reopen PCB tab before DRC so vias register")
    ap.add_argument("--json", dest="json_out", default=None, help="Write DRC report JSON to file")
    args = ap.parse_args()

    if args.autoroute and args.routes:
        res = run_offline_drc(args.autoroute, args.routes, args.min_width)
    elif args.bridge_url:
        res = run_bridge_drc(args.bridge_url, args.pcb_uuid, args.project_uuid, args.reopen)
    else:
        ap.error("Provide either `--autoroute <file> --routes <file>` (offline) or `--bridge-url <url>` (live)")
        return

    print(json.dumps(res, ensure_ascii=False, indent=2))
    if args.json_out:
        with open(args.json_out, "w", encoding="utf-8") as f:
            json.dump(res, f, ensure_ascii=False, indent=2)

    has_err = bool(res.get("violations")) or res.get("nonConnCount", 0) > 0
    sys.exit(1 if has_err else 0)


if __name__ == "__main__":
    main()
