#!/usr/bin/env python3
"""Zero-dependency SVG PCB Route Visualizer for EasyTrace-rs.

Renders JLCEDA `autoroute.json` (board outline, prohibited cutout/NPTH slot regions, rotated/mirrored
component pads, pre-existing copper) + `routes.json` (color-coded multi-layer octilinear traces and vias)
into a crisp, zoomable `.svg` image using only Python's standard library.

Usage:
  python3 scripts/render_svg.py --autoroute autoroute.json --routes routes.json -o board_routes.svg
  python3 scripts/render_svg.py --autoroute autoroute.json --routes routes.json -o panels.svg --split-layers
"""
import argparse
import html
import json
import math

LAYER_COLORS = {
    1: ("Top (L1)", "#ff4d4f"),
    15: ("Inner1 (L15)", "#36cfc9"),
    16: ("Inner2 (L16)", "#73d13d"),
    17: ("Inner3 (L17)", "#ffa940"),
    18: ("Inner4 (L18)", "#b37feb"),
    2: ("Bottom (L2)", "#4096ff"),
}


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


def transform_pads(ar: dict) -> list[tuple[set[int], list[tuple[float, float]]]]:
    fps = ar.get("footprints", {})
    route_layers = set(ar.get("layers", {}).get("route", [1, 2]))
    pads_out = []
    for comp in ar.get("components", {}).values():
        fp = fps.get(comp.get("footprint", ""))
        if not fp:
            continue
        is_bot = comp.get("layer", 1) == 2
        rad = math.radians(comp.get("rotation", 0.0) or 0.0)
        ca, sa = math.cos(rad), math.sin(rad)
        cx, cy = comp.get("location", [0.0, 0.0])

        def tf(lx: float, ly: float) -> tuple[float, float]:
            my = -ly if is_bot else ly
            return (cx + lx * ca - my * sa, cy + lx * sa + my * ca)

        for pad in fp.get("pads", {}).values():
            pl = pad.get("layers", [1])
            if len(pl) > 1 or 12 in pl:
                lays = set(route_layers)
            else:
                lays = {(2 if l == 1 else (1 if l == 2 else l)) if is_bot else l for l in pl}
            local_pts = discretize_path(pad.get("path", []))
            if len(local_pts) >= 3:
                gpts = [tf(px, py) for px, py in local_pts]
            else:
                pcx, pcy = tf(*pad.get("location", [0.0, 0.0]))
                r = (pad.get("diameter") or 0.5) / 2.0
                gpts = [
                    (pcx + r * math.cos(2 * math.pi * k / 16), pcy + r * math.sin(2 * math.pi * k / 16))
                    for k in range(16)
                ]
            pads_out.append((lays, gpts))
    return pads_out


def svg_pts(pts: list[tuple[float, float]], ox: float, oy: float) -> str:
    # Invert Y so +Y points UP just like PCB CAD View!
    return " ".join(f"{ox + x:.3f},{oy - y:.3f}" for x, y in pts)


def main() -> None:
    ap = argparse.ArgumentParser(description="Render PCB + EasyTrace routes to SVG")
    ap.add_argument("--autoroute", required=True, help="Input JLCEDA AutoRouteJson file")
    ap.add_argument("--routes", required=True, help="Input EasyTrace routes.json file")
    ap.add_argument("-o", "--output", required=True, help="Output .svg path")
    ap.add_argument("--title", default=None, help="Optional diagram title")
    ap.add_argument("--split-layers", action="store_true", help="Render one panel per copper layer")
    args = ap.parse_args()

    ar = json.load(open(args.autoroute, encoding="utf-8"))
    rt = json.load(open(args.routes, encoding="utf-8"))

    outer = discretize_path(ar.get("boardOutline", {}).get("path", []))
    bbox = ar.get("boardOutline", {}).get("bbox", [-25.0, 25.0, -25.0, 25.0])
    min_x, max_x, min_y, max_y = bbox[0] - 1.5, bbox[1] + 1.5, bbox[2] - 1.5, bbox[3] + 1.5
    bw, bh = max_x - min_x, max_y - min_y

    raw_layers = ar.get("layers", {}).get("route", [1, 2])
    layers = ([1] if 1 in raw_layers else []) + sorted(l for l in raw_layers if l not in (1, 2)) + ([2] if 2 in raw_layers else [])
    pads = transform_pads(ar)
    prs = [discretize_path(pr.get("path", [])) for pr in ar.get("prohibitedRegions", [])]

    panels = [(l, LAYER_COLORS.get(l, (f"L{l}", "#5cdbd3"))) for l in layers] if args.split_layers else [(None, ("All Layers", "#ffffff"))]
    cols = min(3, len(panels))
    rows = int(math.ceil(len(panels) / cols))
    header_h = 5.0
    total_w = cols * bw
    total_h = header_h + rows * bh
    px_scale = 18.0

    out = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{total_w * px_scale:.0f}" height="{total_h * px_scale:.0f}" '
        f'viewBox="0 0 {total_w:.3f} {total_h:.3f}" style="background:#0f1216;font-family:system-ui,sans-serif;">'
    ]
    st = rt.get("stats", {})
    title = args.title or (
        f"EasyTrace-rs — {st.get('conns_routed', '?')}/{st.get('conns_total', '?')} conns "
        f"({len(rt.get('lines', []))} segs, {len(rt.get('vias', []))} vias, {st.get('elapsed_s', '?')}s)"
    )
    out.append(
        f'<text x="{total_w / 2:.3f}" y="3.2" fill="#e6edf3" font-size="1.8" font-weight="700" text-anchor="middle">'
        f"{html.escape(title)}</text>"
    )

    for idx, (only_layer, (lname, lcol)) in enumerate(panels):
        c_idx, r_idx = idx % cols, idx // cols
        ox = c_idx * bw - min_x
        oy = header_h + r_idx * bh + max_y

        if only_layer is not None:
            out.append(
                f'<text x="{c_idx * bw + 1.5:.3f}" y="{header_h + r_idx * bh + 2.2:.3f}" '
                f'fill="{lcol}" font-size="1.35" font-weight="600">{html.escape(lname)}</text>'
            )

        if len(outer) >= 3:
            out.append(
                f'<polygon points="{svg_pts(outer, ox, oy)}" fill="#161b22" stroke="#8b949e" stroke-width="0.16"/>'
            )
        for pr_pts in prs:
            if len(pr_pts) >= 3:
                out.append(
                    f'<polygon points="{svg_pts(pr_pts, ox, oy)}" fill="#0d1117" stroke="#f85149" '
                    f'stroke-width="0.10" stroke-dasharray="0.35,0.25"/>'
                )
        for p_lays, p_pts in pads:
            if only_layer is None or only_layer in p_lays:
                out.append(
                    f'<polygon points="{svg_pts(p_pts, ox, oy)}" fill="#3b3626" stroke="#8c7a4b" stroke-width="0.04"/>'
                )

        # Pre-existing tracks
        for tr in ar.get("tracks", []):
            tl = tr.get("layer", 1)
            if only_layer is not None and tl != only_layer:
                continue
            tpts = discretize_path(tr.get("path", []))
            if len(tpts) >= 2:
                out.append(
                    f'<polyline points="{svg_pts(tpts, ox, oy)}" fill="none" stroke="#484f58" '
                    f'stroke-width="{tr.get("width", 0.127):.3f}" stroke-linecap="round" stroke-linejoin="round" opacity="0.65"/>'
                )

        # Newly routed EasyTrace segments
        for _, lay, x1, y1, x2, y2, w in rt.get("lines", []):
            if only_layer is not None and lay != only_layer:
                continue
            col = LAYER_COLORS.get(lay, ("", "#5cdbd3"))[1]
            out.append(
                f'<line x1="{ox + x1:.3f}" y1="{oy - y1:.3f}" x2="{ox + x2:.3f}" y2="{oy - y2:.3f}" '
                f'stroke="{col}" stroke-width="{w:.3f}" stroke-linecap="round" opacity="0.92"/>'
            )

        # Vias
        for _, vx, vy, vd in rt.get("vias", []):
            out.append(
                f'<circle cx="{ox + vx:.3f}" cy="{oy - vy:.3f}" r="{vd / 2.0:.3f}" '
                f'fill="#e6c875" stroke="#111" stroke-width="0.04"/>'
                f'<circle cx="{ox + vx:.3f}" cy="{oy - vy:.3f}" r="{min(0.15, vd * 0.28):.3f}" fill="#0d1117"/>'
            )

    out.append("</svg>\n")
    with open(args.output, "w", encoding="utf-8") as f:
        f.write("\n".join(out))
    print(f"Wrote SVG visualization -> {args.output}")


if __name__ == "__main__":
    main()
