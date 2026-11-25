//! EasyTrace-rs v3 PathFinder negotiated-congestion PCB autorouting engine.
//!
//! Architectural highlights:
//! - Exact orthogonal (`st_ortho`, `+0.004 mm`) + half-shifted cell-center diagonal (`st_mid`,
//!   `+0.004 mm`) static obstacle maps: eliminates the `0.071 mm` diagonal phantom bloat and unlocks
//!   `0.35 mm` QFN and `0.40 mm`-pitch WLCSP-9 center-ball escapes.
//! - Split dynamic congestion maps `dy_ortho` (`+0.004 mm`) vs `dy_diag` (`+margin`), plus exact
//!   vertex-aligned `vst` / `vdy` (`+0.004 mm` without margin bloat), giving +20% parallel routing
//!   channel capacity (`0.250 mm` pitch = 5 cells for `0.127 mm` traces).
//! - Same-net fanout via-near-pad unlock: paints `vcl + vr` with `net` so foreign nets keep full
//!   clearance while painting `0x4000_0000 | net` ONLY with `via_hole_d / 2.0 + 0.02`, allowing
//!   same-net fanout vias right next to QFN/0402 pad toes without drill-in-pad solder wicking.
//! - 3-class adaptive squeeze neckdown (`[0.095, 0.127, 0.200] mm`): power nets (`0.200 mm`) auto-neck
//!   to `0.127 mm` within `0.55 mm` of pads, and any net can step down to `0.095 mm` (+0.45 penalty)
//!   only across statically pinched cells.
//! - Per-branch Steiner rip-up & reroute, periodic incomplete-net retry (every 3rd iteration),
//!   per-join expansion caps, non-destructive single-branch legalization, targeted rip-and-reroute
//!   rescue pass, clean tiny-pad micro-stub snap, and octilinear (`0/45/90 deg`) pull-tight smoothing.

use crate::model::{
    in_poly, seg_dist, shape_of_item, Input, LineSeg, RouteConfig, RouteOutput, RouteStats, Rules,
    Shape, ViaOut,
};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashSet};
use std::time::Instant;

const HARD: u16 = 30000;
const EPS_EXACT: f64 = 0.004;

#[derive(Clone)]
struct Map {
    cnt: Vec<u16>,
    sum: Vec<u32>,
}
impl Map {
    fn new(n: usize) -> Map {
        Map {
            cnt: vec![0; n],
            sum: vec![0; n],
        }
    }
    #[inline]
    fn clean(&self, i: usize, net: u32) -> bool {
        let c = self.cnt[i];
        c == 0 || (c == 1 && self.sum[i] == net)
    }
}

#[derive(Clone, Copy)]
struct Grid {
    x0: f64,
    y0: f64,
    h: f64,
    nx: usize,
    ny: usize,
}
impl Grid {
    #[inline]
    fn p(&self, i: usize, j: usize) -> (f64, f64) {
        (self.x0 + i as f64 * self.h, self.y0 + j as f64 * self.h)
    }
    fn range(&self, b: (f64, f64, f64, f64), r: f64) -> (usize, usize, usize, usize) {
        let f = |v: f64, o: f64, n: usize| (((v - o) / self.h).floor().max(0.0) as usize).min(n - 1);
        let g = |v: f64, o: f64, n: usize| (((v - o) / self.h).ceil().max(0.0) as usize).min(n - 1);
        (
            f(b.0 - r, self.x0, self.nx),
            g(b.1 + r, self.x0, self.nx),
            f(b.2 - r, self.y0, self.ny),
            g(b.3 + r, self.y0, self.ny),
        )
    }
}

fn paint(
    m: &mut Map,
    g: &Grid,
    shapes: &[(Shape, f64)],
    net: u32,
    add: bool,
    stamp: &mut [u32],
    sid: &mut u32,
) {
    if shapes.is_empty() {
        return;
    }
    *sid = sid.wrapping_add(1);
    if *sid == 0 {
        stamp.fill(0);
        *sid = 1;
    }
    let s = *sid;
    for (sh, r) in shapes {
        let (i0, i1, j0, j1) = g.range(sh.bbox(), *r);
        for j in j0..=j1 {
            for i in i0..=i1 {
                let k = j * g.nx + i;
                if stamp[k] == s {
                    continue;
                }
                if sh.dist(g.p(i, j)) <= *r {
                    stamp[k] = s;
                    if m.cnt[k] >= HARD {
                        continue;
                    }
                    if add {
                        m.cnt[k] += 1;
                        m.sum[k] = m.sum[k].wrapping_add(net);
                    } else {
                        m.cnt[k] = m.cnt[k].saturating_sub(1);
                        m.sum[k] = m.sum[k].wrapping_sub(net);
                    }
                }
            }
        }
    }
}

fn paint_hard(m: &mut Map, g: &Grid, shapes: &[(Shape, f64)]) {
    for (sh, r) in shapes {
        let (i0, i1, j0, j1) = g.range(sh.bbox(), *r);
        for j in j0..=j1 {
            for i in i0..=i1 {
                let k = j * g.nx + i;
                if sh.dist(g.p(i, j)) <= *r {
                    m.cnt[k] = HARD;
                }
            }
        }
    }
}

#[derive(Copy, Clone, PartialEq)]
struct HeapE {
    f: f32,
    g: f32,
    node: u32,
}
impl Eq for HeapE {}
impl Ord for HeapE {
    fn cmp(&self, o: &Self) -> Ordering {
        o.f.partial_cmp(&self.f)
            .unwrap_or(Ordering::Equal)
            .then_with(|| self.g.partial_cmp(&o.g).unwrap_or(Ordering::Equal))
    }
}
impl PartialOrd for HeapE {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

const D8: [(i32, i32); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (1, -1),
    (-1, 1),
    (-1, -1),
];

#[derive(Default, Clone)]
struct Branch {
    isl_a: usize,
    isl_b: usize,
    segs: Vec<(usize, (f64, f64), (f64, f64), f64)>, // (layer_idx, a, b, width)
    vias: Vec<(f64, f64)>,
    nodes: Vec<(u32, u8, bool)>, // (node, effective_cls, is_diag_step)
    via_cells: Vec<u32>,
    path: Vec<(u32, u8, bool)>,
    painted: bool,
}

#[derive(Default, Clone)]
struct NetRoute {
    branches: Vec<Branch>,
    fails: Vec<String>,
    static_unreach: HashSet<usize>,
}

struct Maps {
    g: Grid,
    g_mid: Grid,
    nl: usize,
    nc: usize,
    rules: Rules,
    margin: f64,
    std_cls: usize,
    squeeze_ok: bool,
    squeeze_pen: f32,
    st_ortho: Vec<Vec<Map>>,
    st_mid: Vec<Vec<Map>>,
    wall: Vec<Vec<Vec<u8>>>,
    dy_ortho: Vec<Vec<Map>>,
    dy_diag: Vec<Vec<Map>>,
    vst: Map,
    vdy: Map,
    hist: Vec<f32>,
    vhist: Vec<f32>,
    stamp: Vec<u32>,
    sid: u32,
}

struct Buf {
    gs: Vec<f32>,
    gst: Vec<u32>,
    par: Vec<u32>,
    dir: Vec<u8>,
    ec_used: Vec<u8>,
    gsid: u32,
    own: Vec<u32>,
    osid: u32,
    neck: Vec<u32>,
    nsid: u32,
    sisl: Vec<u16>,
    tgt: Vec<u32>,
    tisl: Vec<u16>,
    tsid: u32,
    own_vias: Vec<(f64, f64, usize)>,
    expanded: u64,
}
impl Buf {
    fn new(nn: usize, nc: usize) -> Buf {
        Buf {
            gs: vec![0.0; nn],
            gst: vec![0; nn],
            par: vec![0; nn],
            dir: vec![8; nn],
            ec_used: vec![0; nn],
            gsid: 0,
            own: vec![0; nn],
            osid: 0,
            neck: vec![0; nc],
            nsid: 0,
            sisl: vec![0; nn],
            tgt: vec![0; nn],
            tisl: vec![0; nn],
            tsid: 0,
            own_vias: Vec::new(),
            expanded: 0,
        }
    }
}

#[derive(Clone, Copy)]
struct Mode {
    pres: f32,
    hard: bool,
    astar: bool,
    no_hist: bool,
    max_exp: u64,
}

struct BranchShapes {
    per_ortho: Vec<Vec<Vec<(Shape, f64)>>>,
    per_diag: Vec<Vec<Vec<(Shape, f64)>>>,
    vias: Vec<(Shape, f64)>,
}

impl Maps {
    fn branch_shapes(&self, br: &Branch) -> BranchShapes {
        let ru = &self.rules;
        let vr = ru.via_d / 2.0;
        // Exact vertex-aligned via-to-via clearance (+EPS_EXACT, NO diagonal margin bloat!)
        let vv_r = (ru.clr_other + vr + EPS_EXACT).max(ru.via_hole_d + ru.hole_clr + 0.002 - vr);
        let ncls = ru.widths.len();
        let mut per_ortho = vec![vec![Vec::new(); self.nl]; ncls];
        let mut per_diag = vec![vec![Vec::new(); self.nl]; ncls];
        let mut vs = Vec::new();

        for &(l, a, b, sw) in &br.segs {
            let sh = Shape::Seg(a, b, sw / 2.0);
            for (c, wc) in ru.widths.iter().enumerate() {
                per_ortho[c][l].push((sh.clone(), ru.clr_tt + wc / 2.0 + EPS_EXACT));
                per_diag[c][l].push((sh.clone(), ru.clr_tt + wc / 2.0 + self.margin));
            }
            vs.push((
                sh,
                (ru.clr_other + vr + EPS_EXACT).max(ru.via_hole_d / 2.0 + 0.18),
            ));
        }
        for p in &br.vias {
            let sh = Shape::Circ(*p, vr);
            for (c, wc) in ru.widths.iter().enumerate() {
                let tr_v = (ru.clr_other + wc / 2.0).max(ru.via_hole_d / 2.0 + 0.18 + wc / 2.0 - vr);
                for l in 0..self.nl {
                    per_ortho[c][l].push((sh.clone(), tr_v + EPS_EXACT));
                    per_diag[c][l].push((sh.clone(), tr_v + self.margin));
                }
            }
            vs.push((sh, vv_r));
        }
        BranchShapes {
            per_ortho,
            per_diag,
            vias: vs,
        }
    }

    fn paint_branch(&mut self, br: &mut Branch, net: u32, add: bool) {
        if br.painted == add {
            return;
        }
        let bs = self.branch_shapes(br);
        for c in 0..bs.per_ortho.len() {
            for l in 0..self.nl {
                if !bs.per_ortho[c][l].is_empty() {
                    let mut mo = std::mem::replace(&mut self.dy_ortho[c][l], Map::new(0));
                    paint(
                        &mut mo,
                        &self.g,
                        &bs.per_ortho[c][l],
                        net,
                        add,
                        &mut self.stamp,
                        &mut self.sid,
                    );
                    self.dy_ortho[c][l] = mo;

                    let mut md = std::mem::replace(&mut self.dy_diag[c][l], Map::new(0));
                    paint(
                        &mut md,
                        &self.g,
                        &bs.per_diag[c][l],
                        net,
                        add,
                        &mut self.stamp,
                        &mut self.sid,
                    );
                    self.dy_diag[c][l] = md;
                }
            }
        }
        if !bs.vias.is_empty() {
            let mut mv = std::mem::replace(&mut self.vdy, Map::new(0));
            paint(
                &mut mv,
                &self.g,
                &bs.vias,
                net,
                add,
                &mut self.stamp,
                &mut self.sid,
            );
            self.vdy = mv;
        }
        br.painted = add;
    }

    fn paint_net(&mut self, nr: &mut NetRoute, net: u32, add: bool) {
        for br in &mut nr.branches {
            self.paint_branch(br, net, add);
        }
    }

    fn branch_conflicts(&self, br: &Branch) -> usize {
        let mut n = 0;
        for &(node, ec, is_diag) in &br.nodes {
            let (l, i) = (node as usize / self.nc, node as usize % self.nc);
            let dm = if is_diag {
                &self.dy_diag[ec as usize][l]
            } else {
                &self.dy_ortho[ec as usize][l]
            };
            if dm.cnt[i] > 0 {
                n += 1;
            }
        }
        for &i in &br.via_cells {
            if self.vdy.cnt[i as usize] > 0 {
                n += 2;
            }
        }
        n
    }

    fn heur(&self, node: u32, bbs: &[(i32, i32, i32, i32)]) -> f32 {
        let idx = node as usize % self.nc;
        let (i, j) = ((idx % self.g.nx) as i32, (idx / self.g.nx) as i32);
        let mut best = f32::MAX;
        for b in bbs {
            let dx = (b.0 - i).max(0).max(i - b.1) as f32;
            let dy = (b.2 - j).max(0).max(j - b.3) as f32;
            let h = (dx.max(dy) + 0.4142 * dx.min(dy)) * 0.96;
            if h < best {
                best = h;
            }
        }
        best
    }

    #[inline]
    fn pref_cls(&self, b: &Buf, cls: usize, idx: usize) -> usize {
        if cls > self.std_cls && b.neck[idx] == b.nsid {
            self.std_cls
        } else {
            cls
        }
    }

    #[inline]
    fn step_static_ok(
        &self,
        b: &Buf,
        ec: usize,
        l: usize,
        k: usize,
        idx: usize,
        nk: usize,
        ni: usize,
        is_diag: bool,
        mid_i: usize,
        f1: usize,
        f2: usize,
        net: u32,
    ) -> bool {
        // HARD board/cutout/slot keepout is NEVER bypassed even inside b.own!
        if self.st_ortho[ec][l].cnt[ni] >= HARD {
            return false;
        }
        if is_diag {
            if self.st_mid[ec][l].cnt[mid_i] >= HARD {
                return false;
            }
            if b.own[nk] != b.osid {
                if !self.st_ortho[ec][l].clean(ni, net)
                    || (b.own[k] != b.osid && !self.st_ortho[ec][l].clean(idx, net))
                    || !self.st_mid[ec][l].clean(mid_i, net)
                {
                    return false;
                }
                if !self.st_ortho[ec][l].clean(f1, net) && !self.st_ortho[ec][l].clean(f2, net) {
                    return false;
                }
            } else if b.own[k] != b.osid && !self.st_mid[ec][l].clean(mid_i, net) {
                return false;
            }
        } else if b.own[nk] != b.osid && !self.st_ortho[ec][l].clean(ni, net) {
            return false;
        }
        true
    }

    fn search(
        &self,
        b: &mut Buf,
        net: u32,
        cls: usize,
        src: &[u32],
        bbs: &[(i32, i32, i32, i32)],
        md: Mode,
        win: (i32, i32, i32, i32),
    ) -> (Option<Vec<(u32, u8, bool)>>, bool) {
        b.gsid = b.gsid.wrapping_add(1);
        if b.gsid == 0 {
            b.gst.fill(0);
            b.gsid = 1;
        }
        let gs = b.gsid;
        let mut heap = BinaryHeap::new();
        let nx = self.g.nx as i32;
        let hw_mul: f32 = if md.hard { 1.05 } else { 1.60 };
        let hf = |s: &Maps, n: u32| if md.astar { s.heur(n, bbs) * hw_mul } else { 0.0 };
        for &s in src {
            let k = s as usize;
            if b.gst[k] == gs {
                continue;
            }
            let ci = k % self.nc;
            b.gst[k] = gs;
            b.gs[k] = 0.0;
            b.par[k] = u32::MAX;
            b.dir[k] = 8;
            b.ec_used[k] = self.pref_cls(b, cls, ci) as u8;
            heap.push(HeapE {
                f: hf(self, s),
                g: 0.0,
                node: s,
            });
        }
        let vc = (self.rules.via_cost as f32).max(22.0);
        let bend = self.rules.bend_cost as f32;
        let min_same_via = self.rules.via_hole_d + self.rules.hole_clr + 0.002;
        let mut local_exp: u64 = 0;

        while let Some(HeapE { g, node, .. }) = heap.pop() {
            let k = node as usize;
            if g > b.gs[k] + 1e-4 {
                continue;
            }
            local_exp += 1;
            b.expanded += 1;
            if local_exp > md.max_exp {
                return (None, true);
            }
            if b.tgt[k] == b.tsid {
                let mut p = Vec::new();
                let mut c = node;
                loop {
                    let ck = c as usize;
                    let d = b.dir[ck];
                    let is_diag = (4..8).contains(&d);
                    p.push((c, b.ec_used[ck], is_diag));
                    if b.par[ck] == u32::MAX {
                        break;
                    }
                    c = b.par[ck];
                }
                return (Some(p), false);
            }
            let l = k / self.nc;
            let idx = k % self.nc;
            let (i, j) = ((idx % self.g.nx) as i32, (idx / self.g.nx) as i32);
            let d0 = b.dir[k];

            for (dk, (di, dj)) in D8.iter().enumerate() {
                let (a, bb) = (i + di, j + dj);
                if a < win.0 || a > win.1 || bb < win.2 || bb > win.3 {
                    continue;
                }
                let ni = (bb * nx + a) as usize;
                let nk = l * self.nc + ni;
                let is_diag = *di != 0 && *dj != 0;
                let mid_i = if is_diag {
                    (j.min(bb) * nx + i.min(a)) as usize
                } else {
                    0
                };
                let (f1, f2) = if is_diag {
                    ((j * nx + a) as usize, (bb * nx + i) as usize)
                } else {
                    (0, 0)
                };

                let pref_ec = self.pref_cls(b, cls, ni);
                let (ec, sq_pen) = if self.step_static_ok(
                    b, pref_ec, l, k, idx, nk, ni, is_diag, mid_i, f1, f2, net,
                ) {
                    (pref_ec, 0.0f32)
                } else if self.squeeze_ok
                    && pref_ec > 0
                    && self.step_static_ok(b, 0, l, k, idx, nk, ni, is_diag, mid_i, f1, f2, net)
                {
                    (0usize, self.squeeze_pen)
                } else {
                    continue;
                };

                let occ = if is_diag {
                    self.dy_diag[ec][l].cnt[ni] as f32
                } else {
                    self.dy_ortho[ec][l].cnt[ni] as f32
                };
                if md.hard && occ > 0.0 {
                    continue;
                }
                let outer_mul = if self.nl >= 4 && (l == 0 || l + 1 == self.nl) { 1.28 } else { 1.0 };
                let base_step = (if is_diag { 1.4142 } else { 1.0 }) * outer_mul;
                let wall_disc = if self.wall[ec][l][ni] > 0 { 0.965 } else { 1.0 };
                let dir_pen = if self.nl >= 4 && l > 0 && l + 1 < self.nl {
                    if (l % 2 == 1 && *dj != 0) || (l % 2 == 0 && *di != 0) {
                        0.16
                    } else {
                        0.0
                    }
                } else {
                    0.0
                };
                let h_term = if md.no_hist { 0.0 } else { self.hist[nk] };
                let mut c = base_step * wall_disc
                    + sq_pen
                    + dir_pen
                    + base_step * (h_term + md.pres * occ);
                if d0 != 8 && d0 as usize != dk {
                    c += bend;
                }
                let ng = g + c;
                if b.gst[nk] != gs || ng < b.gs[nk] {
                    b.gst[nk] = gs;
                    b.gs[nk] = ng;
                    b.par[nk] = node;
                    b.dir[nk] = dk as u8;
                    b.ec_used[nk] = ec as u8;
                    heap.push(HeapE {
                        f: ng + hf(self, nk as u32),
                        g: ng,
                        node: nk as u32,
                    });
                }
            }

            if self.vst.cnt[idx] < HARD && self.vst.clean(idx, net) {
                let occ = self.vdy.cnt[idx] as f32;
                if !(md.hard && occ > 0.0) {
                    let vp = self.g.p(idx % self.g.nx, idx / self.g.nx);
                    let mut same_reuse = false;
                    let mut too_close_same = false;
                    for &(ox, oy, oidx) in &b.own_vias {
                        if oidx == idx {
                            same_reuse = true;
                            break;
                        }
                        if (vp.0 - ox).hypot(vp.1 - oy) < min_same_via {
                            too_close_same = true;
                            break;
                        }
                    }
                    if !too_close_same {
                        let pref_ec = self.pref_cls(b, cls, idx);
                        let vh_term = if md.no_hist { 0.0 } else { self.vhist[idx] };
                        let vcost_base = if same_reuse {
                            vc * 0.25
                        } else {
                            vc + vc * 0.85 * (vh_term + md.pres * occ)
                        };
                        for l2 in 0..self.nl {
                            if l2 == l {
                                continue;
                            }
                            let nk = l2 * self.nc + idx;
                            let (ec, sq_pen) = if self.st_ortho[pref_ec][l2].cnt[idx] < HARD
                                && (b.own[nk] == b.osid || self.st_ortho[pref_ec][l2].clean(idx, net))
                            {
                                (pref_ec, 0.0f32)
                            } else if self.squeeze_ok
                                && pref_ec > 0
                                && self.st_ortho[0][l2].cnt[idx] < HARD
                                && (b.own[nk] == b.osid || self.st_ortho[0][l2].clean(idx, net))
                            {
                                (0usize, self.squeeze_pen)
                            } else {
                                continue;
                            };
                            if md.hard && self.dy_ortho[ec][l2].cnt[idx] > 0 {
                                continue;
                            }
                            let ng = g + vcost_base + sq_pen;
                            if b.gst[nk] != gs || ng < b.gs[nk] {
                                b.gst[nk] = gs;
                                b.gs[nk] = ng;
                                b.par[nk] = node;
                                b.dir[nk] = 8;
                                b.ec_used[nk] = ec as u8;
                                heap.push(HeapE {
                                    f: ng + hf(self, nk as u32),
                                    g: ng,
                                    node: nk as u32,
                                });
                            }
                        }
                    }
                }
            }
        }
        (None, false)
    }
}

struct NetCtx {
    id: u32,
    name: String,
    cls: usize,
    isl_cells: Vec<Vec<u32>>,
    isl_centers: Vec<Vec<(usize, (f64, f64), Shape)>>,
    plane: bool,
    static_vias: Vec<(f64, f64, usize)>,
}

fn bbox_cells(cells: &[u32], nc: usize, nx: usize) -> (i32, i32, i32, i32) {
    let mut b = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
    for &c in cells {
        let idx = c as usize % nc;
        let (i, j) = ((idx % nx) as i32, (idx / nx) as i32);
        b = (b.0.min(i), b.1.max(i), b.2.min(j), b.3.max(j));
    }
    b
}

fn mark_neck(b: &mut Buf, m: &Maps, n: &NetCtx) {
    if n.cls <= m.std_cls {
        return;
    }
    b.nsid = b.nsid.wrapping_add(1);
    if b.nsid == 0 {
        b.neck.fill(0);
        b.nsid = 1;
    }
    let ns = b.nsid;
    let rad = (0.55 / m.g.h).ceil() as i32;
    let (nx, ny) = (m.g.nx as i32, m.g.ny as i32);
    for (ii, isl) in n.isl_cells.iter().enumerate() {
        if n.plane && ii == 0 {
            continue;
        }
        for &c in isl {
            let idx = c as usize % m.nc;
            let (ci, cj) = ((idx % m.g.nx) as i32, (idx / m.g.nx) as i32);
            for dj in -rad..=rad {
                let jj = cj + dj;
                if jj < 0 || jj >= ny {
                    continue;
                }
                for di in -rad..=rad {
                    if di * di + dj * dj > rad * rad {
                        continue;
                    }
                    let ii2 = ci + di;
                    if ii2 < 0 || ii2 >= nx {
                        continue;
                    }
                    b.neck[(jj * nx + ii2) as usize] = ns;
                }
            }
        }
    }
}

fn build_branch(
    m: &Maps,
    n: &NetCtx,
    p: Vec<(u32, u8, bool)>,
    isl_a: usize,
    isl_b: usize,
) -> Branch {
    let (nc, nx) = (m.nc, m.g.nx);
    let (gx0, gy0, gh) = (m.g.x0, m.g.y0, m.g.h);
    let xy = |idx: usize| (gx0 + (idx % nx) as f64 * gh, gy0 + (idx / nx) as f64 * gh);
    let pts: Vec<(usize, usize, f64)> = p
        .iter()
        .map(|&(c, ec, _)| {
            (
                (c as usize) / nc,
                (c as usize) % nc,
                m.rules.widths[ec as usize],
            )
        })
        .collect();
    let mut br = Branch {
        isl_a,
        isl_b,
        nodes: p.clone(),
        path: p,
        ..Default::default()
    };
    let mut s = 0;
    for t in 1..pts.len() {
        if pts[t].0 != pts[t - 1].0 {
            if t - 1 > s {
                push_run_br(&mut br, &pts[s..t], &xy);
            }
            let v = xy(pts[t].1);
            if !br
                .vias
                .iter()
                .any(|w| (w.0 - v.0).abs() < 1e-6 && (w.1 - v.1).abs() < 1e-6)
            {
                br.vias.push(v);
                br.via_cells.push(pts[t].1 as u32);
            }
            s = t;
        }
    }
    if pts.len() > 1 && pts.len() - 1 > s {
        push_run_br(&mut br, &pts[s..], &xy);
    }
    maybe_snap_pad_stub(m, n, &mut br);
    br
}

fn push_run_br(br: &mut Branch, run: &[(usize, usize, f64)], xy: &dyn Fn(usize) -> (f64, f64)) {
    let l = run[0].0;
    let mut a = 0;
    let d = |u: usize, v: usize| {
        let (p, q) = (xy(run[u].1), xy(run[v].1));
        ((q.0 - p.0) * 1e4).round() as i64 * 1_000_000 + ((q.1 - p.1) * 1e4).round() as i64
    };
    for t in 1..run.len() {
        // Segment from t-1 to t uses the width chosen at step destination (note `p` is target->source,
        // so `run[t-1].2.min(run[t].2)` safely honors squeeze width on the exact pinch step).
        let w_seg = run[a].2.min(run[t].2);
        let same_w = (run[t].2 - run[a].2).abs() < 1e-6;
        if same_w
            && t + 1 < run.len()
            && (run[t + 1].2 - run[a].2).abs() < 1e-6
            && d(t - 1, t) == d(t, t + 1)
        {
            continue;
        }
        br.segs.push((l, xy(run[a].1), xy(run[t].1), w_seg));
        a = t;
    }
}

/// Only emit off-grid pad centroid snap when `sh.dist(ep) > -0.02 && sh.dist(ep) <= 0.02`
/// (tiny pad fallback), avoiding dangling redundant micro-stubs inside normal pads.
fn maybe_snap_pad_stub(m: &Maps, n: &NetCtx, br: &mut Branch) {
    if br.path.is_empty() {
        return;
    }
    let (nc, nx) = (m.nc, m.g.nx);
    let xy = |c: u32| {
        let idx = (c as usize) % nc;
        (
            m.g.x0 + (idx % nx) as f64 * m.g.h,
            m.g.y0 + (idx / nx) as f64 * m.g.h,
        )
    };
    let std_w = m.rules.widths[m.std_cls];
    for &(end_node, end_ec, _) in &[br.path[0], *br.path.last().unwrap()] {
        let l = (end_node as usize) / nc;
        let ep = xy(end_node);
        let sw = m.rules.widths[end_ec as usize];
        for pads in &n.isl_centers {
            for (pl, pc, sh) in pads {
                if *pl != l {
                    continue;
                }
                let dist = (ep.0 - pc.0).hypot(ep.1 - pc.1);
                let ep_d = sh.dist(ep);
                if dist > 1e-4
                    && dist <= 0.14
                    && sh.dist(*pc) <= -0.02
                    && ep_d > -0.02
                    && ep_d <= 0.02
                {
                    br.segs.push((l, ep, *pc, sw.min(std_w)));
                }
            }
        }
    }
}

/// True geometric connected components over `n.isl_cells` (0..k_tot) and `nr.branches` (k_tot..) by shared grid cells.
/// Prevents stale cached `(isl_a, isl_b)` labels from hiding disconnected islands after ripping intermediate Steiner branches.
fn true_uf(n: &NetCtx, nr: &NetRoute) -> Vec<usize> {
    let k_tot = n.isl_cells.len();
    let total = k_tot + nr.branches.len();
    let mut par: Vec<usize> = (0..total).collect();
    fn uf_find(par: &mut [usize], x: usize) -> usize {
        let mut r = x;
        while par[r] != r {
            r = par[r];
        }
        let mut c = x;
        while par[c] != r {
            let p = par[c];
            par[c] = r;
            c = p;
        }
        r
    }
    let mut owner: std::collections::HashMap<u32, usize> = std::collections::HashMap::with_capacity(256);
    for (ii, isl) in n.isl_cells.iter().enumerate() {
        for &c in isl {
            if let Some(&prev) = owner.get(&c) {
                let (a, b) = (uf_find(&mut par, prev), uf_find(&mut par, ii));
                if a != b {
                    par[b] = a;
                }
            } else {
                owner.insert(c, ii);
            }
        }
    }
    for (bi, br) in nr.branches.iter().enumerate() {
        let bid = k_tot + bi;
        for &(c, _, _) in &br.nodes {
            if let Some(&prev) = owner.get(&c) {
                let (a, b) = (uf_find(&mut par, prev), uf_find(&mut par, bid));
                if a != b {
                    par[b] = a;
                }
            } else {
                owner.insert(c, bid);
            }
        }
    }
    for i in 0..total {
        uf_find(&mut par, i);
    }
    par
}

fn route_missing(
    m: &Maps,
    b: &mut Buf,
    n: &NetCtx,
    nr: &mut NetRoute,
    md: Mode,
    winm: i32,
    no_full: bool,
) {
    nr.fails.clear();
    let k_tot = n.isl_cells.len();
    if k_tot < 2 {
        return;
    }
    let (nc, nx) = (m.nc, m.g.nx);
    mark_neck(b, m, n);
    b.osid = b.osid.wrapping_add(1);
    if b.osid == 0 {
        b.own.fill(0);
        b.osid = 1;
    }
    b.own_vias = n.static_vias.clone();

    let par = true_uf(n, nr);
    for br in &nr.branches {
        for &(c, _, _) in &br.nodes {
            b.own[c as usize] = b.osid;
        }
        for &vc in &br.via_cells {
            let vp = m.g.p((vc as usize) % nx, (vc as usize) / nx);
            if !b.own_vias.iter().any(|x| x.2 == vc as usize) {
                b.own_vias.push((vp.0, vp.1, vc as usize));
            }
        }
    }
    for (ii, isl) in n.isl_cells.iter().enumerate() {
        let r = par[ii];
        for &c in isl {
            b.own[c as usize] = b.osid;
            b.sisl[c as usize] = r as u16;
        }
    }
    for (bi, br) in nr.branches.iter().enumerate() {
        let r = par[k_tot + bi];
        for &(c, _, _) in &br.nodes {
            b.sisl[c as usize] = r as u16;
        }
    }

    let mut isl_roots: HashSet<usize> = HashSet::new();
    let mut comp_map: BTreeMap<usize, Vec<u32>> = BTreeMap::new();
    for ii in 0..k_tot {
        let r = par[ii];
        isl_roots.insert(r);
        comp_map.entry(r).or_default().extend_from_slice(&n.isl_cells[ii]);
    }
    for (bi, br) in nr.branches.iter().enumerate() {
        let r = par[k_tot + bi];
        if isl_roots.contains(&r) && !(n.plane && r == par[0]) {
            comp_map
                .entry(r)
                .or_default()
                .extend(br.nodes.iter().map(|x| x.0));
        }
    }
    if comp_map.len() <= 1 {
        return;
    }

    let mut comps: Vec<(usize, Vec<u32>)> = comp_map.into_iter().collect();
    if n.plane {
        let p_root = par[0];
        let p_pos = comps.iter().position(|x| x.0 == p_root).unwrap();
        let (_, plane_cells) = comps.remove(p_pos);
        let pb = bbox_cells(&plane_cells, nc, nx);
        for (cr, c_cells) in comps {
            b.tsid = b.tsid.wrapping_add(1);
            if b.tsid == 0 {
                b.tgt.fill(0);
                b.tsid = 1;
            }
            for &c in &plane_cells {
                b.tgt[c as usize] = b.tsid;
                b.tisl[c as usize] = 0;
            }
            let sb = bbox_cells(&c_cells, nc, nx);
            let win = (
                sb.0.saturating_sub(winm).max(0),
                sb.1.saturating_add(winm).min(nx as i32 - 1),
                sb.2.saturating_sub(winm).max(0),
                sb.3.saturating_add(winm).min(m.g.ny as i32 - 1),
            );
            if let (Some(p), _) = m.search(b, n.id, n.cls, &c_cells, &[pb], md, win) {
                let br = build_branch(m, n, p, cr, 0);
                for &(c, _, _) in &br.nodes {
                    b.own[c as usize] = b.osid;
                }
                for &vc in &br.via_cells {
                    let vp = m.g.p((vc as usize) % nx, (vc as usize) / nx);
                    if !b.own_vias.iter().any(|x| x.2 == vc as usize) {
                        b.own_vias.push((vp.0, vp.1, vc as usize));
                    }
                }
                nr.branches.push(br);
            } else {
                nr.fails.push(format!("island{}", cr));
            }
        }
        return;
    }

    let mut active_comps = Vec::new();
    for (r, cs) in comps {
        if nr.static_unreach.contains(&r) {
            nr.fails.push(format!("island{}", r));
        } else {
            active_comps.push((r, cs));
        }
    }
    if active_comps.len() <= 1 {
        return;
    }

    let (_, mut tree) = active_comps.remove(0);
    let mut rest: Vec<(usize, Vec<u32>, (i32, i32, i32, i32))> = active_comps
        .into_iter()
        .map(|(r, cs)| {
            let bb = bbox_cells(&cs, nc, nx);
            (r, cs, bb)
        })
        .collect();

    while !rest.is_empty() {
        b.tsid = b.tsid.wrapping_add(1);
        if b.tsid == 0 {
            b.tgt.fill(0);
            b.tsid = 1;
        }
        for (q, (_, cs, _)) in rest.iter().enumerate() {
            for &c in cs {
                b.tgt[c as usize] = b.tsid;
                b.tisl[c as usize] = q as u16;
            }
        }

        // Pick nearest remaining island `best_q` + closest source point `(near_i, near_j)` in `tree`
        // -> 6-10x faster inner `heur()` loop (single bbox) & tight local search window!
        let mut best_q = 0usize;
        let mut best_dist = i32::MAX;
        let mut near_ij = (0i32, 0i32);
        let step_stride = (tree.len() / 256).max(1);
        for (ti_idx, &tc) in tree.iter().enumerate() {
            if ti_idx % step_stride != 0 && ti_idx + 1 != tree.len() {
                continue;
            }
            let tidx = tc as usize % nc;
            let (ti, tj) = ((tidx % nx) as i32, (tidx / nx) as i32);
            for (q, (_, _, bb)) in rest.iter().enumerate() {
                let d = (bb.0 - ti).max(0).max(ti - bb.1) + (bb.2 - tj).max(0).max(tj - bb.3);
                if d < best_dist {
                    best_dist = d;
                    best_q = q;
                    near_ij = (ti, tj);
                }
            }
        }
        let target_bb = rest[best_q].2;
        let local_box = (
            near_ij.0.min(target_bb.0),
            near_ij.0.max(target_bb.1),
            near_ij.1.min(target_bb.2),
            near_ij.1.max(target_bb.3),
        );
        let wins: &[i32] = if no_full {
            &[winm]
        } else {
            &[winm, i32::MAX / 4]
        };
        let mut found = None;
        let mut exhausted_full_static = false;
        for &w in wins {
            let win = (
                local_box.0.saturating_sub(w).max(0),
                local_box.1.saturating_add(w).min(nx as i32 - 1),
                local_box.2.saturating_sub(w).max(0),
                local_box.3.saturating_add(w).min(m.g.ny as i32 - 1),
            );
            let (p_opt, hit_cap) = m.search(b, n.id, n.cls, &tree, &[target_bb], md, win);
            if let Some(p) = p_opt {
                found = Some(p);
                break;
            }
            if !md.hard && w >= i32::MAX / 8 && !hit_cap && nr.branches.is_empty() {
                exhausted_full_static = true;
            }
        }
        match found {
            None => {
                let (r_fail, cs_next, _) = rest.remove(best_q);
                if exhausted_full_static && rest.is_empty() {
                    // `tree` exhausted its entire static connected component without reaching `r_fail`!
                    nr.static_unreach.insert(r_fail);
                }
                nr.fails.push(format!("island{}", r_fail));
                tree.extend_from_slice(&cs_next);
            }
            Some(p) => {
                let q = b.tisl[p[0].0 as usize] as usize;
                let (dst_isl, dst_cells, _) = rest.remove(q);
                let src_node = p.last().unwrap().0;
                let src_isl = b.sisl[src_node as usize] as usize;
                let br = build_branch(m, n, p, src_isl, dst_isl);
                for &(c, _, _) in &br.nodes {
                    b.own[c as usize] = b.osid;
                    b.sisl[c as usize] = src_isl as u16;
                }
                for &c in &dst_cells {
                    b.sisl[c as usize] = src_isl as u16;
                }
                for &vc in &br.via_cells {
                    let vp = m.g.p((vc as usize) % nx, (vc as usize) / nx);
                    if !b.own_vias.iter().any(|x| x.2 == vc as usize) {
                        b.own_vias.push((vp.0, vp.1, vc as usize));
                    }
                }
                tree.extend(br.nodes.iter().map(|x| x.0));
                tree.extend_from_slice(&dst_cells);
                nr.branches.push(br);
            }
        }
    }
}

fn measure_branches(
    m: &mut Maps,
    nets: &[NetCtx],
    res: &mut [NetRoute],
    hist_inc: f32,
) -> Vec<(usize, usize, usize)> {
    let mut out = Vec::new();
    for k in 0..nets.len() {
        if res[k].branches.is_empty() {
            continue;
        }
        m.paint_net(&mut res[k], nets[k].id, false);
        for (bi, br) in res[k].branches.iter().enumerate() {
            let c = m.branch_conflicts(br);
            if c > 0 {
                out.push((k, bi, c));
                if hist_inc > 0.0 {
                    for &(node, ec, is_diag) in &br.nodes {
                        let (l, i) = (node as usize / m.nc, node as usize % m.nc);
                        let dm = if is_diag {
                            &m.dy_diag[ec as usize][l]
                        } else {
                            &m.dy_ortho[ec as usize][l]
                        };
                        if dm.cnt[i] > 0 {
                            m.hist[node as usize] += hist_inc;
                        }
                    }
                    for &vc in &br.via_cells {
                        if m.vdy.cnt[vc as usize] > 0 {
                            m.vhist[vc as usize] += hist_inc * 1.5;
                        }
                    }
                }
            }
        }
        m.paint_net(&mut res[k], nets[k].id, true);
    }
    out
}

fn net_joined_count(n: &NetCtx, nr: &NetRoute) -> usize {
    let k_tot = n.isl_cells.len();
    if k_tot < 2 {
        return 0;
    }
    let par = true_uf(n, nr);
    let mut roots: HashSet<usize> = HashSet::new();
    for i in 0..k_tot {
        roots.insert(par[i]);
    }
    k_tot.saturating_sub(roots.len())
}

fn total_joined_conns(nets: &[NetCtx], res: &[NetRoute]) -> usize {
    (0..nets.len())
        .map(|k| net_joined_count(&nets[k], &res[k]))
        .sum()
}

fn prog(t0: &Instant, nets: &[NetCtx], res: &[NetRoute], bad_nets: &HashSet<usize>, quiet: bool) {
    if quiet {
        return;
    }
    let (mut c, mut n) = (0, 0);
    for k in 0..nets.len() {
        if bad_nets.contains(&k) {
            continue;
        }
        let j = net_joined_count(&nets[k], &res[k]);
        let need = nets[k].isl_cells.len().saturating_sub(1);
        c += j;
        if j >= need && res[k].fails.is_empty() {
            n += 1;
        }
    }
    println!("PROG {:.1} {} {}", t0.elapsed().as_secs_f64(), c, n);
}

pub fn route_input(mut inp: Input, cfg: &RouteConfig) -> RouteOutput {
    let t0 = Instant::now();

    // Default `widths = [0.127, 0.200]` (`5 mil` default min width in JLCEDA Pro).
    // Only prepend a squeeze class (e.g. `0.095 mm`) if `cfg.min_width` is explicitly requested
    // OR the input JSON explicitly placed `widths[0] <= 0.105`.
    if inp.rules.widths.is_empty() {
        inp.rules.widths = vec![0.127, 0.200];
    }
    let (std_cls, squeeze_ok) = if inp.rules.widths[0] <= 0.105 && inp.rules.widths.len() >= 2 {
        (1usize, true)
    } else if let Some(mw) = cfg.min_width {
        let sq_w = mw.min(inp.rules.widths[0]);
        if sq_w + 1e-4 < inp.rules.widths[0] {
            inp.rules.widths.insert(0, sq_w);
            for n in &mut inp.nets {
                n.cls += 1;
            }
            (1usize, true)
        } else {
            (0usize, false)
        }
    } else {
        (0usize, false)
    };

    let h = inp.h;
    let eps_diag = h * std::f64::consts::SQRT_2 / 4.0 + 1e-3;
    let nl = inp.layers.len();
    let (mut x0, mut x1, mut y0, mut y1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for p in &inp.board.outer {
        x0 = x0.min(p[0]);
        x1 = x1.max(p[0]);
        y0 = y0.min(p[1]);
        y1 = y1.max(p[1]);
    }
    if x0 > x1 {
        x0 = -10.0;
        x1 = 10.0;
        y0 = -10.0;
        y1 = 10.0;
    }
    let gx0 = (x0 / h).floor() * h;
    let gy0 = (y0 / h).floor() * h;
    let nx = ((x1 - x0) / h) as usize + 3;
    let ny = ((y1 - y0) / h) as usize + 3;
    let g = Grid {
        x0: gx0,
        y0: gy0,
        h,
        nx,
        ny,
    };
    let g_mid = Grid {
        x0: gx0 + 0.5 * h,
        y0: gy0 + 0.5 * h,
        h,
        nx,
        ny,
    };
    let nc = g.nx * g.ny;
    let nn = nc * nl;
    let ru = inp.rules.clone();
    let margin = h * std::f64::consts::SQRT_2 / 2.0 + 1e-4;
    let ncls = ru.widths.len();

    let mut m = Maps {
        g,
        g_mid,
        nl,
        nc,
        margin,
        std_cls,
        squeeze_ok,
        squeeze_pen: cfg.squeeze_penalty,
        st_ortho: vec![vec![Map::new(nc); nl]; ncls],
        st_mid: vec![vec![Map::new(nc); nl]; ncls],
        wall: vec![vec![vec![0u8; nc]; nl]; ncls],
        dy_ortho: vec![vec![Map::new(nc); nl]; ncls],
        dy_diag: vec![vec![Map::new(nc); nl]; ncls],
        vst: Map::new(nc),
        vdy: Map::new(nc),
        hist: vec![0.0; nn],
        vhist: vec![0.0; nc],
        stamp: vec![0; nc],
        sid: 0,
        rules: ru,
    };

    // ---- Board outer outline vs inner cutout holes (separate edge vs slot_track / slot_via!)
    let outer_ring: Vec<(f64, f64)> = inp.board.outer.iter().map(|p| (p[0], p[1])).collect();
    let hole_rings: Vec<Vec<(f64, f64)>> = inp
        .board
        .holes
        .iter()
        .map(|r| r.iter().map(|p| (p[0], p[1])).collect())
        .collect();
    let vr = m.rules.via_d / 2.0;

    if outer_ring.len() >= 3 {
        for j in 0..m.g.ny {
            for i in 0..m.g.nx {
                let k = j * m.g.nx + i;
                // 1) Grid vertex `(i, j)` for `st_ortho` and `vst`
                let p = m.g.p(i, j);
                let inside = in_poly(p, &outer_ring) && !hole_rings.iter().any(|hh| in_poly(p, hh));
                if !inside {
                    for c in 0..ncls {
                        for l in 0..nl {
                            m.st_ortho[c][l].cnt[k] = HARD;
                        }
                    }
                    m.vst.cnt[k] = HARD;
                } else {
                    let mut d_out = f64::MAX;
                    for e in 0..outer_ring.len() {
                        d_out = d_out.min(seg_dist(
                            p,
                            outer_ring[e],
                            outer_ring[(e + 1) % outer_ring.len()],
                        ));
                    }
                    let mut d_hole = f64::MAX;
                    for hh in &hole_rings {
                        if hh.len() >= 2 {
                            for e in 0..hh.len() {
                                d_hole = d_hole.min(seg_dist(p, hh[e], hh[(e + 1) % hh.len()]));
                            }
                        }
                    }
                    for c in 0..ncls {
                        let hw = m.rules.widths[c] / 2.0;
                        if d_out < m.rules.edge + hw + EPS_EXACT
                            || d_hole < m.rules.slot_track + hw + EPS_EXACT
                        {
                            for l in 0..nl {
                                m.st_ortho[c][l].cnt[k] = HARD;
                            }
                        }
                    }
                    if d_out < m.rules.edge + vr + EPS_EXACT
                        || d_hole < m.rules.slot_via + vr + EPS_EXACT
                    {
                        m.vst.cnt[k] = HARD;
                    }
                }

                // 2) Exact cell-center midpoint `(i + 0.5, j + 0.5)` for `st_mid`
                let pm = m.g_mid.p(i, j);
                let inside_m =
                    in_poly(pm, &outer_ring) && !hole_rings.iter().any(|hh| in_poly(pm, hh));
                if !inside_m {
                    for c in 0..ncls {
                        for l in 0..nl {
                            m.st_mid[c][l].cnt[k] = HARD;
                        }
                    }
                } else {
                    let mut d_out = f64::MAX;
                    for e in 0..outer_ring.len() {
                        d_out = d_out.min(seg_dist(
                            pm,
                            outer_ring[e],
                            outer_ring[(e + 1) % outer_ring.len()],
                        ));
                    }
                    let mut d_hole = f64::MAX;
                    for hh in &hole_rings {
                        if hh.len() >= 2 {
                            for e in 0..hh.len() {
                                d_hole = d_hole.min(seg_dist(pm, hh[e], hh[(e + 1) % hh.len()]));
                            }
                        }
                    }
                    for c in 0..ncls {
                        let hw = m.rules.widths[c] / 2.0;
                        if d_out < m.rules.edge + hw + eps_diag
                            || d_hole < m.rules.slot_track + hw + eps_diag
                        {
                            for l in 0..nl {
                                m.st_mid[c][l].cnt[k] = HARD;
                            }
                        }
                    }
                }
            }
        }
    }

    // ---- Static obstacles (`st_ortho`, `st_mid`, `vst`)
    // First paint HARD `keepout` items so no trace/via ever enters prohibited regions on their layers.
    for it in inp.items.iter().filter(|it| it.kind == "keepout") {
        let s = shape_of_item(it);
        let tcl = if it.clr > 0.0 {
            it.clr
        } else {
            m.rules.slot_track
        };
        for c in 0..ncls {
            let hw = m.rules.widths[c] / 2.0;
            let sh_o = [(s.clone(), tcl + hw + EPS_EXACT)];
            let sh_m = [(s.clone(), tcl + hw + eps_diag)];
            for &l in &it.layers {
                if l < nl {
                    paint_hard(&mut m.st_ortho[c][l], &m.g, &sh_o);
                    paint_hard(&mut m.st_mid[c][l], &m.g_mid, &sh_m);
                }
            }
        }
        if it.vclr >= 0.0 {
            let vcl = if it.vclr > 0.0 {
                it.vclr
            } else {
                m.rules.slot_via
            };
            paint_hard(&mut m.vst, &m.g, &[(s, vcl + vr + EPS_EXACT)]);
        }
    }

    let mut by_net: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (k, it) in inp.items.iter().enumerate() {
        if it.kind == "keepout" {
            continue;
        }
        by_net.entry(it.net).or_default().push(k);
    }

    for (net, ks) in &by_net {
        for c in 0..ncls {
            let wc = m.rules.widths[c];
            for l in 0..nl {
                let mut sh_o = Vec::new();
                let mut sh_m = Vec::new();
                for &k in ks.iter().filter(|&&k| inp.items[k].layers.contains(&l)) {
                    let it = &inp.items[k];
                    let cl = if it.clr > 0.0 {
                        it.clr
                    } else if it.kind == "track" {
                        m.rules.clr_tt
                    } else if it.kind == "slot" {
                        m.rules.slot_track
                    } else {
                        m.rules.clr_other
                    };
                    let s = shape_of_item(it);
                    sh_o.push((s.clone(), cl + wc / 2.0 + EPS_EXACT));
                    sh_m.push((s, cl + wc / 2.0 + eps_diag));
                }
                if !sh_o.is_empty() {
                    let mut mo = std::mem::replace(&mut m.st_ortho[c][l], Map::new(0));
                    paint(
                        &mut mo,
                        &m.g,
                        &sh_o,
                        *net,
                        true,
                        &mut m.stamp,
                        &mut m.sid,
                    );
                    m.st_ortho[c][l] = mo;

                    let mut mmid = std::mem::replace(&mut m.st_mid[c][l], Map::new(0));
                    paint(
                        &mut mmid,
                        &m.g_mid,
                        &sh_m,
                        *net,
                        true,
                        &mut m.stamp,
                        &mut m.sid,
                    );
                    m.st_mid[c][l] = mmid;
                }
            }
        }

        // Same-net fanout via-near-pad unlock on `vst`:
        // 1) Paint full `vcl + vr` (and `hole2Hole` for vias) with normal `*net` for BOTH non-pad
        //    and `pad` items under one stamp so DIFFERENT nets keep full clearance while SAME-NET
        //    vias see `clean(idx, net) == true` right next to QFN/0402 pad toes!
        // 2) Paint `0x4000_0000 | *net` ONLY with `via_hole_d / 2.0 + 0.02` on `pad`s (keeps via
        //    drill outside SMD pad so no solder wicking) and `vcl + vr` on `slot`s!
        let vcl = |k: usize| {
            let it = &inp.items[k];
            if it.vclr > 0.0 {
                it.vclr
            } else if it.kind == "slot" {
                m.rules.slot_via
            } else if it.clr > 0.0 {
                it.clr
            } else {
                m.rules.clr_other
            }
        };

        let sh_same_net: Vec<(Shape, f64)> = ks
            .iter()
            .filter(|&&k| inp.items[k].kind != "slot")
            .map(|&k| {
                let it = &inp.items[k];
                let base = vcl(k) + vr + EPS_EXACT;
                let h2h = if it.kind == "via" {
                    m.rules.via_hole_d + m.rules.hole_clr + 0.002 - it.r
                } else {
                    0.0
                };
                (shape_of_item(it), base.max(h2h))
            })
            .collect();

        let sh_all_block: Vec<(Shape, f64)> = ks
            .iter()
            .filter(|&&k| inp.items[k].kind == "pad" || inp.items[k].kind == "slot")
            .map(|&k| {
                let it = &inp.items[k];
                let r_blk = if it.kind == "pad" {
                    m.rules.via_hole_d / 2.0 + 0.02
                } else {
                    vcl(k) + vr + EPS_EXACT
                };
                (shape_of_item(it), r_blk)
            })
            .collect();

        let mut mm = std::mem::replace(&mut m.vst, Map::new(0));
        if !sh_same_net.is_empty() {
            paint(
                &mut mm,
                &m.g,
                &sh_same_net,
                *net,
                true,
                &mut m.stamp,
                &mut m.sid,
            );
        }
        if !sh_all_block.is_empty() {
            paint(
                &mut mm,
                &m.g,
                &sh_all_block,
                0x4000_0000 | *net,
                true,
                &mut m.stamp,
                &mut m.sid,
            );
        }
        m.vst = mm;
    }

    // Precompute wall-hugging micro-bonus flags
    for c in 0..ncls {
        for l in 0..nl {
            if m.g.ny > 2 && m.g.nx > 2 {
                for j in 1..m.g.ny - 1 {
                    for i in 1..m.g.nx - 1 {
                        let k = j * m.g.nx + i;
                        if m.st_ortho[c][l].cnt[k] > 0 {
                            continue;
                        }
                        if m.st_ortho[c][l].cnt[k - 1] > 0
                            || m.st_ortho[c][l].cnt[k + 1] > 0
                            || m.st_ortho[c][l].cnt[k - m.g.nx] > 0
                            || m.st_ortho[c][l].cnt[k + m.g.nx] > 0
                        {
                            m.wall[c][l][k] = 1;
                        }
                    }
                }
            }
        }
    }

    for &l in &cfg.block_layers {
        if l < nl {
            for c in 0..ncls {
                for k in 0..nc {
                    m.st_ortho[c][l].cnt[k] = HARD;
                    m.st_mid[c][l].cnt[k] = HARD;
                }
            }
        }
    }

    for n in &inp.nets {
        if let Some(&l) = cfg.planes.get(&n.name) {
            if l < nl {
                for c in 0..ncls {
                    for k in 0..nc {
                        if m.st_ortho[c][l].cnt[k] == 0 {
                            m.st_ortho[c][l].cnt[k] = 1;
                            m.st_ortho[c][l].sum[k] = n.id;
                        }
                        if m.st_mid[c][l].cnt[k] == 0 {
                            m.st_mid[c][l].cnt[k] = 1;
                            m.st_mid[c][l].sum[k] = n.id;
                        }
                    }
                }
            }
        }
    }

    // ---- Terminal cells of every island
    let mut nets: Vec<NetCtx> = Vec::new();
    let w_min = m.rules.widths[0];
    for n in &inp.nets {
        let mut isl_cells = Vec::new();
        let mut isl_centers = Vec::new();
        let mut static_vias = Vec::new();
        for &k in by_net.get(&n.id).into_iter().flatten() {
            if inp.items[k].kind == "via" {
                let c = inp.items[k].c;
                let gi = ((c[0] - m.g.x0) / h).round().max(0.0) as usize;
                let gj = ((c[1] - m.g.y0) / h).round().max(0.0) as usize;
                static_vias.push((
                    c[0],
                    c[1],
                    gj.min(m.g.ny - 1) * m.g.nx + gi.min(m.g.nx - 1),
                ));
            }
        }
        for isl in &n.islands {
            let mut cells = Vec::new();
            let mut centers = Vec::new();
            for &k in isl {
                let it = &inp.items[k];
                let sh = shape_of_item(it);
                if it.kind == "pad" && (it.center[0] != 0.0 || it.center[1] != 0.0) {
                    for &l in &it.layers {
                        if l < nl {
                            centers.push((l, (it.center[0], it.center[1]), sh.clone()));
                        }
                    }
                }
                let (i0, i1, j0, j1) = m.g.range(sh.bbox(), 0.0);
                let need = match it.kind.as_str() {
                    "pad" => -(w_min / 2.0 - 0.015).min(0.05),
                    _ => -0.01,
                };
                let mut best = (f64::MAX, 0usize);
                let before = cells.len();
                for j in j0..=j1 {
                    for i in i0..=i1 {
                        let idx = j * m.g.nx + i;
                        if it
                            .layers
                            .iter()
                            .any(|&l| l < nl && m.st_ortho[0][l].cnt[idx] >= HARD)
                        {
                            continue;
                        }
                        let d = sh.dist(m.g.p(i, j));
                        if d < best.0 {
                            best = (d, idx);
                        }
                        if d <= need {
                            for &l in &it.layers {
                                if l < nl {
                                    cells.push((l * nc + idx) as u32);
                                }
                            }
                        }
                    }
                }
                if cells.len() == before && it.kind == "pad" && best.0 < 0.02 {
                    for &l in &it.layers {
                        if l < nl {
                            cells.push((l * nc + best.1) as u32);
                        }
                    }
                }
            }
            cells.sort_unstable();
            cells.dedup();
            if !cells.is_empty() {
                isl_cells.push(cells);
                isl_centers.push(centers);
            }
        }
        let plane = cfg.planes.get(&n.name).copied();
        if let Some(l) = plane {
            if l < nl {
                let pc: Vec<u32> = (0..nc)
                    .filter(|&k| m.st_ortho[n.cls.min(ncls - 1)][l].clean(k, n.id))
                    .map(|k| (l * nc + k) as u32)
                    .collect();
                isl_cells.insert(0, pc);
                isl_centers.insert(0, Vec::new());
            }
        }
        nets.push(NetCtx {
            id: n.id,
            name: n.name.clone(),
            cls: n.cls.min(ncls - 1),
            isl_cells,
            isl_centers,
            plane: plane.is_some(),
            static_vias,
        });
    }

    let nconn: usize = nets
        .iter()
        .map(|n| n.isl_cells.len().saturating_sub(1))
        .sum();
    if !cfg.quiet {
        eprintln!(
            "easytrace v0.2 ({}) ; grid {}x{}x{} (layers {:?}, widths {:?}) h={} ; {} nets, {} connections ; setup {:.2}s",
            cfg.algo,
            m.g.nx,
            m.g.ny,
            nl,
            inp.layers,
            m.rules.widths,
            h,
            nets.len(),
            nconn,
            t0.elapsed().as_secs_f64()
        );
    }
    prog(&t0, &nets, &vec![NetRoute::default(); nets.len()], &HashSet::new(), cfg.quiet);

    let winm = (cfg.window_mm / h) as i32;
    let gnx = m.g.nx;
    let mut order: Vec<usize> = (0..nets.len()).collect();
    let span = move |n: &NetCtx| -> i64 {
        if n.plane || n.isl_cells.is_empty() {
            return 0;
        }
        let b = bbox_cells(&n.isl_cells.concat(), nc, gnx);
        ((b.1 - b.0) + (b.3 - b.2)) as i64
    };
    order.sort_by_key(|&k| span(&nets[k]));

    let mut res: Vec<NetRoute> = vec![NetRoute::default(); nets.len()];
    let mut buf = Buf::new(nn, nc);

    if cfg.algo == "lee" || cfg.algo == "solo" {
        let md = Mode {
            pres: 0.0,
            hard: true,
            astar: cfg.algo == "solo",
            no_hist: false,
            max_exp: cfg.max_exp_hard,
        };
        for (q, &k) in order.iter().enumerate() {
            let mut nr = NetRoute::default();
            route_missing(&m, &mut buf, &nets[k], &mut nr, md, winm, cfg.no_full);
            if cfg.algo == "solo" {
                if !cfg.quiet {
                    eprintln!(
                        "solo {} joined {}/{} fails {:?}",
                        nets[k].name,
                        net_joined_count(&nets[k], &nr),
                        nets[k].isl_cells.len().saturating_sub(1),
                        nr.fails
                    );
                }
            } else {
                m.paint_net(&mut nr, nets[k].id, true);
            }
            res[k] = nr;
            if q % 8 == 7 {
                prog(&t0, &nets, &res, &HashSet::new(), cfg.quiet);
            }
        }
        prog(&t0, &nets, &res, &HashSet::new(), cfg.quiet);
    } else {
        let mut pres = cfg.pres0;
        let pmul = cfg.pres_mul;
        let hinc = cfg.hist_inc;

        // Initial pass: route all nets with per-join cap `max_exp`
        let md0 = Mode {
            pres,
            hard: false,
            astar: true,
            no_hist: false,
            max_exp: cfg.max_exp,
        };
        for &k in &order {
            route_missing(&m, &mut buf, &nets[k], &mut res[k], md0, winm, cfg.no_full);
            m.paint_net(&mut res[k], nets[k].id, true);
        }

        for it in 0..cfg.iters {
            let bad = measure_branches(&mut m, &nets, &mut res, hinc);
            let bad_nets: HashSet<usize> = bad.iter().map(|x| x.0).collect();
            let bad_cells: usize = bad.iter().map(|x| x.2).sum();
            let retry_incomplete = it % 3 == 2;
            let has_incomplete = (0..nets.len()).any(|k| {
                !res[k].fails.is_empty()
                    || net_joined_count(&nets[k], &res[k]) < nets[k].isl_cells.len().saturating_sub(1)
            });

            if !cfg.quiet {
                eprintln!(
                    "iter {:2}: conflicting branches {:3} across {:2} nets ({} cells), pres {:.2}, {:.1}s, expanded {:.1}M",
                    it,
                    bad.len(),
                    bad_nets.len(),
                    bad_cells,
                    pres,
                    t0.elapsed().as_secs_f64(),
                    buf.expanded as f64 / 1_000_000.0
                );
            }
            prog(&t0, &nets, &res, &bad_nets, cfg.quiet);
            if t0.elapsed().as_secs_f64() > cfg.budget_s {
                break;
            }
            if bad.is_empty() && (!has_incomplete || it >= 3) {
                break;
            }

            pres = (pres * pmul).min(400.0);
            let md = Mode {
                pres,
                hard: false,
                astar: true,
                no_hist: false,
                max_exp: cfg.max_exp,
            };

            let mut by_k: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
            for &(k, bi, _) in &bad {
                by_k.entry(k).or_default().push(bi);
            }
            // Retry incomplete nets every 3rd iteration (skipping nets whose remaining islands are statically walled off!)
            if retry_incomplete || bad.is_empty() {
                for k in 0..nets.len() {
                    let need = nets[k].isl_cells.len().saturating_sub(1);
                    let joined = net_joined_count(&nets[k], &res[k]);
                    let rem = need.saturating_sub(joined);
                    if rem > 0 && res[k].static_unreach.len() < rem {
                        by_k.entry(k).or_default();
                    }
                }
            }

            let mut todo_k: Vec<usize> = by_k.keys().copied().collect();
            todo_k.sort_by_key(|&k| span(&nets[k]));
            for k in todo_k {
                let mut bis = by_k.remove(&k).unwrap();
                bis.sort_unstable();
                bis.dedup();
                for &bi in bis.iter().rev() {
                    let mut old_br = res[k].branches.remove(bi);
                    m.paint_branch(&mut old_br, nets[k].id, false);
                }
                m.paint_net(&mut res[k], nets[k].id, false);
                route_missing(&m, &mut buf, &nets[k], &mut res[k], md, winm, cfg.no_full);
                m.paint_net(&mut res[k], nets[k].id, true);
            }
        }

        // ---- Non-destructive per-branch legalization: peel ONLY the worst conflicting single branch
        let mut peeled_nets = BTreeSet::new();
        loop {
            let bad = measure_branches(&mut m, &nets, &mut res, 0.0);
            match bad.iter().max_by_key(|x| x.2) {
                None => break,
                Some(&(k, bi, _)) => {
                    let mut old_br = res[k].branches.remove(bi);
                    m.paint_branch(&mut old_br, nets[k].id, false);
                    peeled_nets.insert(k);
                }
            }
        }
        prog(&t0, &nets, &res, &HashSet::new(), cfg.quiet);

        // ---- Re-try all incomplete / peeled nets with `hard: true` (`max_exp_hard`)
        let md_hard = Mode {
            pres: 0.0,
            hard: true,
            astar: true,
            no_hist: false,
            max_exp: cfg.max_exp_hard,
        };
        for &k in &order {
            let need = nets[k].isl_cells.len().saturating_sub(1);
            if net_joined_count(&nets[k], &res[k]) >= need && !peeled_nets.contains(&k) {
                continue;
            }
            m.paint_net(&mut res[k], nets[k].id, false);
            route_missing(
                &m,
                &mut buf,
                &nets[k],
                &mut res[k],
                md_hard,
                winm,
                cfg.no_full,
            );
            m.paint_net(&mut res[k], nets[k].id, true);
        }
        prog(&t0, &nets, &res, &HashSet::new(), cfg.quiet);

        // ---- Targeted rip-and-reroute rescue pass:
        // For any still-incomplete net after `hard: true`, route with `hard: false, pres: 200.0, no_hist: true`,
        // rip ONLY the 1..=3 conflicting foreign branches, commit, and re-route displaced foreign
        // nets with `hard: true` — revert cleanly if total legal connections don't strictly improve!
        let md_rescue = Mode {
            pres: 200.0,
            hard: false,
            astar: true,
            no_hist: true,
            max_exp: cfg.max_exp,
        };
        for &k in &order {
            if t0.elapsed().as_secs_f64() > cfg.budget_s {
                break;
            }
            let need = nets[k].isl_cells.len().saturating_sub(1);
            let before_k = net_joined_count(&nets[k], &res[k]);
            if before_k >= need {
                continue;
            }

            let base_total = total_joined_conns(&nets, &res);
            m.paint_net(&mut res[k], nets[k].id, false);
            let snap_k = res[k].clone();
            route_missing(
                &m,
                &mut buf,
                &nets[k],
                &mut res[k],
                md_rescue,
                winm,
                cfg.no_full,
            );
            let after_k = net_joined_count(&nets[k], &res[k]);
            if after_k <= before_k {
                res[k] = snap_k;
                m.paint_net(&mut res[k], nets[k].id, true);
                continue;
            }
            m.paint_net(&mut res[k], nets[k].id, true);

            let bad = measure_branches(&mut m, &nets, &mut res, 0.0);
            let foreign_bad: Vec<(usize, usize)> = bad
                .iter()
                .filter(|&&(fk, _, _)| fk != k)
                .map(|&(fk, fbi, _)| (fk, fbi))
                .collect();

            if foreign_bad.len() > 3 {
                m.paint_net(&mut res[k], nets[k].id, false);
                res[k] = snap_k;
                m.paint_net(&mut res[k], nets[k].id, true);
                continue;
            }

            // Unpaint BEFORE cloning `sv` so all cloned branches have `painted == false`,
            // then remove ONLY the 1..=3 conflicting foreign branches and re-route with `hard: true`.
            let mut by_fk: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
            for (fk, fbi) in foreign_bad {
                by_fk.entry(fk).or_default().push(fbi);
            }
            let mut orig_foreign: BTreeMap<usize, NetRoute> = BTreeMap::new();
            for (&fk, fbis) in &mut by_fk {
                m.paint_net(&mut res[fk], nets[fk].id, false);
                let sv = res[fk].clone();
                orig_foreign.insert(fk, sv);
                fbis.sort_unstable();
                fbis.dedup();
                for &fbi in fbis.iter().rev() {
                    res[fk].branches.remove(fbi);
                }
            }

            for &fk in by_fk.keys() {
                route_missing(
                    &m,
                    &mut buf,
                    &nets[fk],
                    &mut res[fk],
                    md_hard,
                    winm,
                    cfg.no_full,
                );
                m.paint_net(&mut res[fk], nets[fk].id, true);
            }

            // Verify zero residual conflicts and strictly higher total connections
            let rem_bad = measure_branches(&mut m, &nets, &mut res, 0.0);
            let new_total = total_joined_conns(&nets, &res);
            if rem_bad.is_empty() && new_total > base_total {
                if !cfg.quiet {
                    eprintln!(
                        "rescue {}: connections {} -> {} (displaced {} foreign nets)",
                        nets[k].name,
                        base_total,
                        new_total,
                        by_fk.len()
                    );
                }
            } else {
                // Revert cleanly (`snap_k` and `sv` already have `painted == false`!)
                m.paint_net(&mut res[k], nets[k].id, false);
                res[k] = snap_k;
                m.paint_net(&mut res[k], nets[k].id, true);
                for (fk, sv) in orig_foreign {
                    m.paint_net(&mut res[fk], nets[fk].id, false);
                    res[fk] = sv;
                    m.paint_net(&mut res[fk], nets[fk].id, true);
                }
            }
        }
        prog(&t0, &nets, &res, &HashSet::new(), cfg.quiet);
    }

    // ---- Default octilinear (`0/45/90 deg`) pull-tight smoothing
    if !cfg.no_smooth {
        let (s0, l0) = seg_stats(&res);
        for &k in &order {
            smooth_net(&mut m, &nets[k], &mut res[k]);
        }
        let (s1, l1) = seg_stats(&res);
        if !cfg.quiet {
            eprintln!(
                "smooth: segments {} -> {}, length {:.1} -> {:.1} mm",
                s0, s1, l0, l1
            );
        }
    }

    // ---- Assemble output
    let lid = &inp.layers;
    let mut segs = Vec::new();
    let mut vias = Vec::new();
    let mut fail = BTreeMap::new();
    let rd = |v: f64| (v * 1e4).round() / 1e4;
    let mut length_mm = 0.0;

    for (k, n) in nets.iter().enumerate() {
        let mut seen_v: Vec<(f64, f64)> = Vec::new();
        for br in &res[k].branches {
            for &(l, a, b, sw) in &br.segs {
                let len = (a.0 - b.0).hypot(a.1 - b.1);
                if len < 1e-5 {
                    continue;
                }
                length_mm += len;
                segs.push(LineSeg {
                    net: n.name.clone(),
                    layer: lid[l],
                    x1: rd(a.0),
                    y1: rd(a.1),
                    x2: rd(b.0),
                    y2: rd(b.1),
                    width: rd(sw),
                });
            }
            for &p in &br.vias {
                if !seen_v.iter().any(|w| (w.0 - p.0).hypot(w.1 - p.1) < 1e-4) {
                    seen_v.push(p);
                    vias.push(ViaOut {
                        net: n.name.clone(),
                        x: rd(p.0),
                        y: rd(p.1),
                        diameter: m.rules.via_d,
                        hole: m.rules.via_hole_d,
                    });
                }
            }
        }
        if !res[k].fails.is_empty() {
            fail.insert(n.name.clone(), res[k].fails.clone());
        }
    }

    let (mut c_tot, mut n_tot) = (0, 0);
    for k in 0..nets.len() {
        let j = net_joined_count(&nets[k], &res[k]);
        let need = nets[k].isl_cells.len().saturating_sub(1);
        c_tot += j;
        if j >= need && res[k].fails.is_empty() {
            n_tot += 1;
        }
    }

    let elapsed_s = t0.elapsed().as_secs_f64();
    if !cfg.quiet {
        println!("RESULT {} {} {}", c_tot, n_tot, nconn);
        eprintln!(
            "done {:.2}s: {}/{} conns ({}/{} nets), {} segments, {} vias, {} nets w/ fails, {:.1}M expanded",
            elapsed_s,
            c_tot,
            nconn,
            n_tot,
            nets.len(),
            segs.len(),
            vias.len(),
            fail.len(),
            buf.expanded as f64 / 1_000_000.0
        );
    }

    RouteOutput {
        stats: RouteStats {
            elapsed_s: (elapsed_s * 100.0).round() / 100.0,
            nets_total: nets.len(),
            nets_routed: n_tot,
            conns_total: nconn,
            conns_routed: c_tot,
            segments: segs.len(),
            vias: vias.len(),
            length_mm: (length_mm * 100.0).round() / 100.0,
            expanded_nodes: buf.expanded,
            via_d_mm: m.rules.via_d,
            via_hole_mm: m.rules.via_hole_d,
        },
        segs,
        vias,
        fail,
    }
}

fn seg_stats(res: &[NetRoute]) -> (usize, f64) {
    let mut n = 0;
    let mut l = 0.0;
    for r in res {
        for br in &r.branches {
            n += br.segs.len();
            for &(_, a, b, _) in &br.segs {
                l += (a.0 - b.0).hypot(a.1 - b.1);
            }
        }
    }
    (n, l)
}

fn smooth_net(m: &mut Maps, n: &NetCtx, nr: &mut NetRoute) {
    if nr.branches.is_empty() {
        return;
    }
    m.paint_net(nr, n.id, false);
    let (nc, nx) = (m.nc, m.g.nx as i32);
    let ny = m.g.ny as i32;
    let xy = |i: i32, j: i32| (m.g.x0 + i as f64 * m.g.h, m.g.y0 + j as f64 * m.g.h);
    let fixed: HashSet<u32> = nr
        .branches
        .iter()
        .filter(|br| !br.path.is_empty())
        .flat_map(|br| [br.path[0].0, br.path.last().unwrap().0])
        .collect();

    for br in &mut nr.branches {
        if br.path.is_empty() {
            continue;
        }
        let p = &br.path;
        let cut: Vec<(usize, i32, i32, u8)> = p
            .iter()
            .map(|&(c, ec, _)| {
                let l = c as usize / nc;
                let idx = (c as usize % nc) as i32;
                (l, idx % nx, idx / nx, ec)
            })
            .collect();
        let mut segs = Vec::new();
        let mut s = 0;
        for t in 1..=cut.len() {
            let brk = t < cut.len()
                && ((t > s + 1 && fixed.contains(&p[t - 1].0) && cut[t].0 == cut[t - 1].0)
                    || cut[t].3 != cut[t - 1].3);
            if t == cut.len() || cut[t].0 != cut[t - 1].0 || brk {
                if t - 1 > s {
                    let l = cut[s].0;
                    let ec = cut[s].3 as usize;
                    let sw = m.rules.widths[ec];
                    let run: Vec<(i32, i32)> = cut[s..t].iter().map(|c| (c.1, c.2)).collect();
                    let orig: HashSet<(i32, i32)> = run.iter().cloned().collect();
                    let mut orig_edges: HashSet<((i32, i32), (i32, i32))> =
                        HashSet::with_capacity(run.len() * 2);
                    for pair in run.windows(2) {
                        orig_edges.insert((pair[0], pair[1]));
                        orig_edges.insert((pair[1], pair[0]));
                    }

                    let vtx_clean = |a: (i32, i32), is_d: bool, allow_orig: bool| -> bool {
                        if a.0 < 0 || a.1 < 0 || a.0 >= nx || a.1 >= ny {
                            return false;
                        }
                        let k = (a.1 * nx + a.0) as usize;
                        if m.st_ortho[ec][l].cnt[k] >= HARD {
                            return false;
                        }
                        let st_ok = (allow_orig && orig.contains(&a))
                            || m.st_ortho[ec][l].clean(k, n.id);
                        let dy_ok = if is_d {
                            m.dy_diag[ec][l].cnt[k] == 0
                        } else {
                            m.dy_ortho[ec][l].cnt[k] == 0
                        };
                        st_ok && dy_ok
                    };

                    let line = |a: (i32, i32), b: (i32, i32)| -> bool {
                        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
                        if !(dx == 0 || dy == 0 || dx.abs() == dy.abs()) {
                            return false;
                        }
                        let is_d = dx != 0 && dy != 0;
                        let st = dx.abs().max(dy.abs());
                        let (sx, sy) = (dx.signum(), dy.signum());
                        for q in 1..=st {
                            let prev = (a.0 + (q - 1) * sx, a.1 + (q - 1) * sy);
                            let cur = (a.0 + q * sx, a.1 + q * sy);
                            // Exact original edges were already validated during routing!
                            if orig_edges.contains(&(prev, cur)) {
                                continue;
                            }
                            // Any newly introduced shortcut/chamfer edge MUST verify BOTH endpoints
                            // (including `dy_diag` when `is_d`!) AND diagonal midpoint + flanks!
                            if !vtx_clean(prev, is_d, prev == run[0] || prev == *run.last().unwrap())
                                || !vtx_clean(
                                    cur,
                                    is_d,
                                    cur == run[0] || cur == *run.last().unwrap(),
                                )
                            {
                                return false;
                            }
                            if is_d {
                                let mid_i =
                                    (prev.1.min(cur.1) * nx + prev.0.min(cur.0)) as usize;
                                if m.st_mid[ec][l].cnt[mid_i] >= HARD
                                    || !m.st_mid[ec][l].clean(mid_i, n.id)
                                {
                                    return false;
                                }
                                let f1 = (prev.1 * nx + cur.0) as usize;
                                let f2 = (cur.1 * nx + prev.0) as usize;
                                if !m.st_ortho[ec][l].clean(f1, n.id)
                                    && !m.st_ortho[ec][l].clean(f2, n.id)
                                {
                                    return false;
                                }
                            }
                        }
                        true
                    };

                    let connect = |a: (i32, i32), b: (i32, i32)| -> Option<Option<(i32, i32)>> {
                        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
                        if dx == 0 || dy == 0 || dx.abs() == dy.abs() {
                            return if line(a, b) { Some(None) } else { None };
                        }
                        let d = dx.abs().min(dy.abs());
                        let c1 = (a.0 + d * dx.signum(), a.1 + d * dy.signum());
                        let c2 = (b.0 - d * dx.signum(), b.1 - d * dy.signum());
                        for c in [c1, c2] {
                            if vtx_clean(c, true, false) && line(a, c) && line(c, b) {
                                return Some(Some(c));
                            }
                        }
                        None
                    };

                    let mut wp = vec![run[0]];
                    let mut i = 0;
                    let mm = run.len() - 1;
                    while i < mm {
                        let mut j = mm.min(i + 300);
                        loop {
                            if let Some(c) = connect(run[i], run[j]) {
                                if let Some(c) = c {
                                    wp.push(c);
                                }
                                wp.push(run[j]);
                                i = j;
                                break;
                            }
                            j -= 1;
                            if j == i {
                                wp.push(run[i + 1]);
                                i += 1;
                                break;
                            }
                        }
                    }

                    let mut w2: Vec<(i32, i32)> = Vec::new();
                    for q in wp {
                        if w2.len() >= 2 {
                            let (a, b) = (w2[w2.len() - 2], w2[w2.len() - 1]);
                            if (b.0 - a.0) * (q.1 - b.1) == (b.1 - a.1) * (q.0 - b.0)
                                && ((b.0 - a.0) * (q.0 - b.0) + (b.1 - a.1) * (q.1 - b.1)) > 0
                            {
                                w2.pop();
                            }
                        }
                        if w2.last() != Some(&q) {
                            w2.push(q);
                        }
                    }
                    for q in 1..w2.len() {
                        if w2[q] != w2[q - 1] {
                            segs.push((l, xy(w2[q - 1].0, w2[q - 1].1), xy(w2[q].0, w2[q].1), sw));
                        }
                    }
                }
                s = if t < cut.len() && cut[t].0 == cut[t - 1].0 {
                    t - 1
                } else {
                    t
                };
            }
        }
        br.segs = segs;
        maybe_snap_pad_stub(m, n, br);
    }
    m.paint_net(nr, n.id, true);
}
