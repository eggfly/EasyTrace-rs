//! Core geometry, PCB rule/net data structures, and pure-Rust island connectivity.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub fn def_slot_tr() -> f64 {
    0.335
}
pub fn def_slot_via() -> f64 {
    0.19
}
pub fn def_via_hole() -> f64 {
    0.30
}
pub fn def_hole_clr() -> f64 {
    0.305
}
pub fn def_via_cost() -> f64 {
    32.0
}
pub fn def_bend_cost() -> f64 {
    1.8
}
pub fn def_widths() -> Vec<f64> {
    vec![0.127, 0.200]
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Rules {
    pub clr_tt: f64,
    pub clr_other: f64,
    pub edge: f64,
    #[serde(default = "def_slot_tr")]
    pub slot_track: f64,
    #[serde(default = "def_slot_via")]
    pub slot_via: f64,
    pub via_d: f64,
    #[serde(default = "def_via_hole")]
    pub via_hole_d: f64,
    #[serde(default = "def_hole_clr")]
    pub hole_clr: f64,
    #[serde(default = "def_via_cost")]
    pub via_cost: f64,
    #[serde(default = "def_bend_cost")]
    pub bend_cost: f64,
    #[serde(default = "def_widths")]
    pub widths: Vec<f64>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
pub struct Board {
    pub outer: Vec<[f64; 2]>,
    #[serde(default)]
    pub holes: Vec<Vec<[f64; 2]>>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
pub struct Item {
    pub net: u32,
    pub layers: Vec<usize>,
    /// `"pad"`, `"track"`, `"via"`, `"slot"`, or `"keepout"`
    pub kind: String,
    #[serde(default)]
    pub poly: Vec<[f64; 2]>,
    #[serde(default)]
    pub a: [f64; 2],
    #[serde(default)]
    pub b: [f64; 2],
    #[serde(default)]
    pub w: f64,
    #[serde(default)]
    pub c: [f64; 2],
    #[serde(default)]
    pub r: f64,
    #[serde(default)]
    pub clr: f64,
    #[serde(default)]
    pub vclr: f64,
    #[serde(default)]
    pub center: [f64; 2],
    #[serde(default)]
    pub name: String,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct NetIn {
    pub id: u32,
    pub name: String,
    pub width: f64,
    pub cls: usize,
    pub islands: Vec<Vec<usize>>,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Input {
    pub h: f64,
    pub layers: Vec<i32>,
    pub rules: Rules,
    pub items: Vec<Item>,
    pub nets: Vec<NetIn>,
    pub board: Board,
}

#[derive(Clone, Debug)]
pub struct RouteConfig {
    pub algo: String,
    pub iters: usize,
    pub budget_s: f64,
    pub window_mm: f64,
    pub pres0: f32,
    pub pres_mul: f32,
    pub hist_inc: f32,
    pub no_full: bool,
    pub no_smooth: bool,
    /// Optional explicit pinch-throat squeeze width (e.g. `Some(0.095)` via `--min-width 0.095`).
    /// `None` by default so all traces stay `>= 0.127 mm` (JLCEDA Pro default 5 mil rule).
    pub min_width: Option<f64>,
    pub squeeze_penalty: f32,
    pub max_exp: u64,
    pub max_exp_hard: u64,
    pub block_layers: Vec<usize>,
    pub planes: BTreeMap<String, usize>,
    pub quiet: bool,
}

impl Default for RouteConfig {
    fn default() -> Self {
        Self {
            algo: "pf".to_string(),
            iters: 45,
            budget_s: 3600.0,
            window_mm: 3.5,
            pres0: 0.5,
            pres_mul: 1.35,
            hist_inc: 1.0,
            no_full: false,
            no_smooth: false,
            min_width: None,
            squeeze_penalty: 0.45,
            max_exp: 1_500_000,
            max_exp_hard: 1_800_000,
            block_layers: Vec::new(),
            planes: BTreeMap::new(),
            quiet: false,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LineSeg {
    pub net: String,
    pub layer: i32,
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
    pub width: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ViaOut {
    pub net: String,
    pub x: f64,
    pub y: f64,
    pub diameter: f64,
    pub hole: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct RouteStats {
    pub elapsed_s: f64,
    pub nets_total: usize,
    pub nets_routed: usize,
    pub conns_total: usize,
    pub conns_routed: usize,
    pub segments: usize,
    pub vias: usize,
    pub length_mm: f64,
    pub expanded_nodes: u64,
    pub via_d_mm: f64,
    pub via_hole_mm: f64,
}

#[derive(Clone, Debug, Default)]
pub struct RouteOutput {
    pub segs: Vec<LineSeg>,
    pub vias: Vec<ViaOut>,
    pub fail: BTreeMap<String, Vec<String>>,
    pub stats: RouteStats,
}

impl RouteOutput {
    /// Convert to standard EasyTrace/JLCEDA `routes.json` value:
    /// `{"lines": [[net, layer, x1, y1, x2, y2, w], ...], "vias": [[net, x, y, d], ...], "fail": {...}, "stats": {...}}`
    pub fn to_routes_json(&self) -> serde_json::Value {
        let lines: Vec<serde_json::Value> = self
            .segs
            .iter()
            .map(|s| serde_json::json!([s.net, s.layer, s.x1, s.y1, s.x2, s.y2, s.width]))
            .collect();
        let vias: Vec<serde_json::Value> = self
            .vias
            .iter()
            .map(|v| serde_json::json!([v.net, v.x, v.y, v.diameter]))
            .collect();
        serde_json::json!({
            "lines": lines,
            "vias": vias,
            "fail": self.fail,
            "stats": self.stats,
        })
    }
}

#[derive(Clone, Debug)]
pub enum Shape {
    Poly(Vec<(f64, f64)>),
    Seg((f64, f64), (f64, f64), f64),
    Circ((f64, f64), f64),
}

#[inline]
pub fn seg_dist(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let l2 = dx * dx + dy * dy;
    let t = if l2 < 1e-18 {
        0.0
    } else {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / l2).clamp(0.0, 1.0)
    };
    ((p.0 - a.0 - t * dx).powi(2) + (p.1 - a.1 - t * dy).powi(2)).sqrt()
}

pub fn in_poly(p: (f64, f64), pts: &[(f64, f64)]) -> bool {
    if pts.len() < 3 {
        return false;
    }
    let mut c = false;
    let n = pts.len();
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0 {
            c = !c;
        }
    }
    c
}

#[inline]
fn orient(a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> f64 {
    (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
}

#[inline]
fn segments_intersect(a1: (f64, f64), b1: (f64, f64), a2: (f64, f64), b2: (f64, f64)) -> bool {
    let o1 = orient(a1, b1, a2);
    let o2 = orient(a1, b1, b2);
    let o3 = orient(a2, b2, a1);
    let o4 = orient(a2, b2, b1);
    (o1 * o2 < 0.0) && (o3 * o4 < 0.0)
}

pub fn seg_seg_dist(a1: (f64, f64), b1: (f64, f64), a2: (f64, f64), b2: (f64, f64)) -> f64 {
    if segments_intersect(a1, b1, a2, b2) {
        return 0.0;
    }
    seg_dist(a1, a2, b2)
        .min(seg_dist(b1, a2, b2))
        .min(seg_dist(a2, a1, b1))
        .min(seg_dist(b2, a1, b1))
}

impl Shape {
    pub fn dist(&self, p: (f64, f64)) -> f64 {
        match self {
            Shape::Seg(a, b, hw) => seg_dist(p, *a, *b) - hw,
            Shape::Circ(c, r) => ((p.0 - c.0).powi(2) + (p.1 - c.1).powi(2)).sqrt() - r,
            Shape::Poly(pts) => {
                if pts.is_empty() {
                    return f64::MAX;
                }
                let n = pts.len();
                let mut d = f64::MAX;
                for i in 0..n {
                    d = d.min(seg_dist(p, pts[i], pts[(i + 1) % n]));
                }
                if in_poly(p, pts) {
                    -d
                } else {
                    d
                }
            }
        }
    }

    pub fn bbox(&self) -> (f64, f64, f64, f64) {
        match self {
            Shape::Seg(a, b, hw) => (
                a.0.min(b.0) - hw,
                a.0.max(b.0) + hw,
                a.1.min(b.1) - hw,
                a.1.max(b.1) + hw,
            ),
            Shape::Circ(c, r) => (c.0 - r, c.0 + r, c.1 - r, c.1 + r),
            Shape::Poly(pts) => pts.iter().fold(
                (f64::MAX, f64::MIN, f64::MAX, f64::MIN),
                |b, p| (b.0.min(p.0), b.1.max(p.0), b.2.min(p.1), b.3.max(p.1)),
            ),
        }
    }

    pub fn dist_to_shape(&self, other: &Shape) -> f64 {
        match (self, other) {
            (Shape::Circ(c1, r1), Shape::Circ(c2, r2)) => {
                (c1.0 - c2.0).hypot(c1.1 - c2.1) - r1 - r2
            }
            (Shape::Circ(c, r), s) | (s, Shape::Circ(c, r)) => s.dist(*c) - r,
            (Shape::Seg(a1, b1, hw1), Shape::Seg(a2, b2, hw2)) => {
                seg_seg_dist(*a1, *b1, *a2, *b2) - hw1 - hw2
            }
            (Shape::Seg(a, b, hw), Shape::Poly(pts)) | (Shape::Poly(pts), Shape::Seg(a, b, hw)) => {
                if pts.is_empty() {
                    return f64::MAX;
                }
                if in_poly(*a, pts) || in_poly(*b, pts) {
                    return -hw;
                }
                let n = pts.len();
                let mut best = f64::MAX;
                for i in 0..n {
                    let d = seg_seg_dist(*a, *b, pts[i], pts[(i + 1) % n]);
                    if d < best {
                        best = d;
                    }
                    if best == 0.0 {
                        break;
                    }
                }
                best - hw
            }
            (Shape::Poly(p1), Shape::Poly(p2)) => {
                if p1.is_empty() || p2.is_empty() {
                    return f64::MAX;
                }
                if in_poly(p1[0], p2) || in_poly(p2[0], p1) {
                    return -0.01;
                }
                let (n1, n2) = (p1.len(), p2.len());
                let mut best = f64::MAX;
                for i in 0..n1 {
                    let (a1, b1) = (p1[i], p1[(i + 1) % n1]);
                    for j in 0..n2 {
                        let d = seg_seg_dist(a1, b1, p2[j], p2[(j + 1) % n2]);
                        if d < best {
                            best = d;
                        }
                        if best == 0.0 {
                            return 0.0;
                        }
                    }
                }
                best
            }
        }
    }
}

pub fn shape_of_item(it: &Item) -> Shape {
    match it.kind.as_str() {
        "pad" | "keepout" if !it.poly.is_empty() => {
            let mut pts: Vec<(f64, f64)> = it.poly.iter().map(|p| (p[0], p[1])).collect();
            dedup_ring(&mut pts);
            Shape::Poly(pts)
        }
        "track" => Shape::Seg((it.a[0], it.a[1]), (it.b[0], it.b[1]), it.w / 2.0),
        _ => Shape::Circ((it.c[0], it.c[1]), it.r),
    }
}

pub fn dedup_ring(pts: &mut Vec<(f64, f64)>) {
    pts.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6);
    if pts.len() >= 2 {
        let first = pts[0];
        let last = *pts.last().unwrap();
        if (first.0 - last.0).abs() < 1e-6 && (first.1 - last.1).abs() < 1e-6 {
            pts.pop();
        }
    }
}

pub fn poly_centroid(pts: &[(f64, f64)]) -> (f64, f64) {
    if pts.is_empty() {
        return (0.0, 0.0);
    }
    let n = pts.len();
    let mut area2 = 0.0;
    let mut cx = 0.0;
    let mut cy = 0.0;
    for i in 0..n {
        let (x0, y0) = pts[i];
        let (x1, y1) = pts[(i + 1) % n];
        let cross = x0 * y1 - x1 * y0;
        area2 += cross;
        cx += (x0 + x1) * cross;
        cy += (y0 + y1) * cross;
    }
    if area2.abs() < 1e-12 {
        let sx: f64 = pts.iter().map(|p| p.0).sum();
        let sy: f64 = pts.iter().map(|p| p.1).sum();
        (sx / n as f64, sy / n as f64)
    } else {
        (cx / (3.0 * area2), cy / (3.0 * area2))
    }
}

/// Group same-net copper items (`pad`, `track`, `via`) into connected copper islands in pure Rust.
/// Returns only islands containing at least one `"pad"` item, sorted largest-first.
pub fn compute_net_islands(items: &[Item], item_indices: &[usize], touch_tol: f64) -> Vec<Vec<usize>> {
    let m = item_indices.len();
    if m == 0 {
        return Vec::new();
    }
    let shapes: Vec<(Shape, (f64, f64, f64, f64))> = item_indices
        .iter()
        .map(|&idx| {
            let s = shape_of_item(&items[idx]);
            let b = s.bbox();
            (s, b)
        })
        .collect();

    let mut par: Vec<usize> = (0..m).collect();
    fn find(par: &mut [usize], mut x: usize) -> usize {
        while par[x] != x {
            par[x] = par[par[x]];
            x = par[x];
        }
        x
    }

    for a in 0..m {
        let ia = item_indices[a];
        let ba = shapes[a].1;
        for b in (a + 1)..m {
            let ra = find(&mut par, a);
            let rb = find(&mut par, b);
            if ra == rb {
                continue;
            }
            let ib = item_indices[b];
            // Must share at least one copper layer
            if !items[ia].layers.iter().any(|l| items[ib].layers.contains(l)) {
                continue;
            }
            let bb = shapes[b].1;
            if ba.1 + touch_tol < bb.0
                || bb.1 + touch_tol < ba.0
                || ba.3 + touch_tol < bb.2
                || bb.3 + touch_tol < ba.2
            {
                continue;
            }
            if shapes[a].0.dist_to_shape(&shapes[b].0) <= touch_tol {
                par[rb] = ra;
            }
        }
    }

    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for a in 0..m {
        let r = find(&mut par, a);
        groups.entry(r).or_default().push(item_indices[a]);
    }
    let mut islands: Vec<Vec<usize>> = groups
        .into_values()
        .filter(|grp| grp.iter().any(|&idx| items[idx].kind == "pad"))
        .collect();
    islands.sort_by(|a, b| b.len().cmp(&a.len()));
    islands
}

pub fn is_default_power_net(name: &str) -> bool {
    let u = name.to_ascii_uppercase();
    if u == "GND" || u.starts_with("GND") || u == "AGND" || u == "DGND" || u == "PGND" {
        return false;
    }
    for prefix in [
        "VBAT", "VBUS", "VCC", "VDD", "VIN", "VOUT", "VSYS", "VPWR", "AVDD", "DVDD", "+5V", "+3V",
        "+12V", "5V0", "3V3", "1V8",
    ] {
        if u == prefix || u.starts_with(&format!("{}_", prefix)) {
            return true;
        }
    }
    matches!(u.as_str(), "5V" | "3.3V" | "1.8V" | "12V" | "BAT+" | "BATT")
}
