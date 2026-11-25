//! Pure-Rust parser for JLCEDA Pro / EasyEDA Pro native `getAutoRouteJsonFile()` JSON format.
//!
//! Preserves 100% of board geometry that JLCEDA Pro's Specctra `.dsn` exporter drops as keepouts:
//! - Internal board cutouts, tactile switch NPTH bosses (`SW*`), and MEMS mic ports (`prohibitedRegions`)
//! - Exact 1oz/2oz clearance tables (`trackToTrack`, `trackToPad`, `trackToProhibitedRegion`, `viaToProhibitedRegion`)
//! - Exact bottom-layer (`layer == 2`) component `MIRROR_Y` (`ly = -ly`) + CCW rotation transform
//! - Copper pours / fill regions (`fillRegions`) and pre-existing copper (`tracks` & `vias`)

use crate::dsn::{write_ses_string, DsnMeta};
use crate::model::{
    compute_net_islands, dedup_ring, def_bend_cost, def_hole_clr, def_via_cost,
    is_default_power_net, Board, Input, Item, LineSeg, NetIn, RouteOutput, Rules, ViaOut,
};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AutoRouteJson {
    pub board_outline: BoardOutlineJson,
    pub layers: LayersJson,
    #[serde(default)]
    pub rules: RulesJson,
    #[serde(default)]
    pub nets: Vec<NetRuleJson>,
    #[serde(default)]
    pub components: BTreeMap<String, ComponentJson>,
    #[serde(default)]
    pub footprints: BTreeMap<String, FootprintJson>,
    #[serde(default)]
    pub tracks: Vec<TrackJson>,
    #[serde(default)]
    pub vias: Vec<ViaJson>,
    #[serde(default)]
    pub fill_regions: Vec<FillRegionJson>,
    #[serde(default)]
    pub prohibited_regions: Vec<ProhibitedRegionJson>,
}

#[derive(Deserialize, Debug, Default)]
pub struct BoardOutlineJson {
    #[serde(default)]
    pub bbox: [f64; 4],
    /// Each vertex is `[x, y]` or `[end_x, end_y, arc_sweep_deg]`
    #[serde(default)]
    pub path: Vec<Vec<f64>>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct LayersJson {
    #[serde(default)]
    pub route: Vec<i32>,
    #[serde(default)]
    pub not_route: Vec<i32>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct RulesJson {
    #[serde(default)]
    pub safe_clearances: BTreeMap<String, Vec<SafeClearanceJson>>,
    #[serde(default)]
    pub track_widths: BTreeMap<String, Vec<TrackWidthRuleJson>>,
    #[serde(default)]
    pub via_sizes: BTreeMap<String, [f64; 2]>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SafeClearanceJson {
    #[serde(default)]
    pub track_to_track: f64,
    #[serde(default)]
    pub track_to_via: f64,
    #[serde(default)]
    pub track_to_pad: f64,
    #[serde(default)]
    pub track_to_fill_region: f64,
    #[serde(default)]
    pub track_to_prohibited_region: f64,
    #[serde(default)]
    pub track_to_board_outline: f64,
    #[serde(default)]
    pub via_to_via: f64,
    #[serde(default)]
    pub via_to_pad: f64,
    #[serde(default)]
    pub via_to_prohibited_region: f64,
    #[serde(default)]
    pub via_to_board_outline: f64,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TrackWidthRuleJson {
    /// `[minWidth, defaultWidth, maxWidth]` in mm
    #[serde(default)]
    pub track_width: Vec<f64>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct NetRuleJson {
    pub net: String,
    #[serde(default = "bool_true")]
    pub routing: bool,
    #[serde(default)]
    pub safe_clearance: String,
    #[serde(default)]
    pub track_width: String,
    #[serde(default)]
    pub via_size: String,
}

fn bool_true() -> bool {
    true
}

#[derive(Deserialize, Debug, Clone)]
pub struct ComponentJson {
    #[serde(default)]
    pub name: String,
    pub footprint: String,
    #[serde(default = "i32_one")]
    pub layer: i32,
    #[serde(default)]
    pub location: [f64; 2],
    #[serde(default)]
    pub rotation: f64,
    #[serde(default)]
    pub nets: BTreeMap<String, String>,
}

fn i32_one() -> i32 {
    1
}

#[derive(Deserialize, Debug, Clone)]
pub struct FootprintJson {
    #[serde(default)]
    pub pads: BTreeMap<String, PadJson>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct PadJson {
    #[serde(default)]
    pub number: serde_json::Value,
    #[serde(default)]
    pub layers: Vec<i32>,
    #[serde(default)]
    pub location: [f64; 2],
    /// Vertices `[x, y]` or arc segments `[end_x, end_y, sweep_deg]`
    #[serde(default)]
    pub path: Vec<Vec<f64>>,
    #[serde(default)]
    pub diameter: Option<f64>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct TrackJson {
    #[serde(default)]
    pub width: f64,
    #[serde(default = "i32_one")]
    pub layer: i32,
    #[serde(default)]
    pub net: String,
    #[serde(default)]
    pub path: Vec<Vec<f64>>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ViaJson {
    #[serde(default)]
    pub location: [f64; 2],
    #[serde(default)]
    pub size: [f64; 2],
    #[serde(default)]
    pub net: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct FillRegionJson {
    #[serde(default)]
    pub path: Vec<Vec<f64>>,
    #[serde(default = "i32_one")]
    pub layer: i32,
    #[serde(default)]
    pub net: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ProhibitedRegionJson {
    #[serde(default)]
    pub path: Vec<Vec<f64>>,
    #[serde(default)]
    pub layers: Vec<i32>,
}

#[derive(Clone, Debug)]
pub struct JlcedaOptions {
    pub grid_h: f64,
    pub skip_nets: HashSet<String>,
    pub power_nets: HashSet<String>,
    pub auto_power: bool,
    pub ignore_existing: bool,
}

impl Default for JlcedaOptions {
    fn default() -> Self {
        let mut skip = HashSet::new();
        skip.insert("GND".to_string());
        Self {
            grid_h: 0.05,
            skip_nets: skip,
            power_nets: HashSet::new(),
            auto_power: true,
            ignore_existing: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct JlcedaMeta {
    pub bbox: [f64; 4],
    pub layers: Vec<i32>,
    pub via_d: f64,
    pub via_hole_d: f64,
    pub preserved_wires: Vec<LineSeg>,
    pub preserved_vias: Vec<ViaOut>,
}

fn circle_pts(cx: f64, cy: f64, r: f64, n: usize) -> Vec<[f64; 2]> {
    (0..n)
        .map(|k| {
            let a = 2.0 * std::f64::consts::PI * (k as f64) / (n as f64);
            [cx + r * a.cos(), cy + r * a.sin()]
        })
        .collect()
}

/// Discretize JLCEDA `path` where each entry is `[x, y]` (line) or `[ex, ey, sweep_deg]` (arc).
fn discretize_jlceda_path(raw: &[Vec<f64>]) -> Vec<(f64, f64)> {
    let mut pts: Vec<(f64, f64)> = Vec::with_capacity(raw.len() * 2);
    for elem in raw {
        if elem.len() < 2 {
            continue;
        }
        let (ex, ey) = (elem[0], elem[1]);
        if elem.len() >= 3 && !pts.is_empty() {
            let ang_deg = elem[2];
            let (sx, sy) = *pts.last().unwrap();
            let a = ang_deg.to_radians();
            let ch = (ex - sx).hypot(ey - sy);
            if a.abs() < 1e-6 || ch < 1e-7 {
                pts.push((ex, ey));
                continue;
            }
            let sin_half = (a.abs() / 2.0).sin();
            if sin_half.abs() < 1e-7 {
                pts.push((ex, ey));
                continue;
            }
            let r = ch / (2.0 * sin_half);
            let (mx, my) = ((sx + ex) / 2.0, (sy + ey) / 2.0);
            let dd = (r * r - ch * ch / 4.0).max(0.0).sqrt();
            let (ux, uy) = (-(ey - sy) / ch, (ex - sx) / ch);
            let sgn = if (a > 0.0) == (a.abs() < std::f64::consts::PI) {
                1.0
            } else {
                -1.0
            };
            let (cx, cy) = (mx + sgn * ux * dd, my + sgn * uy * dd);
            let a0 = (sy - cy).atan2(sx - cx);
            let steps = ((ang_deg.abs() / 15.0).ceil() as usize).clamp(4, 36);
            for k in 1..=steps {
                let t = a0 + a * (k as f64) / (steps as f64);
                pts.push((cx + r * t.cos(), cy + r * t.sin()));
            }
        } else {
            pts.push((ex, ey));
        }
    }
    pts
}

/// Sort JLCEDA copper layer IDs in physical Z-stackup order: `[1 (Top), 15, 16, ..., 2 (Bottom)]`.
fn canonical_copper_layers(raw: &[i32]) -> Vec<i32> {
    let mut set: Vec<i32> = raw.iter().copied().collect();
    set.sort_unstable();
    set.dedup();
    let mut ordered = Vec::with_capacity(set.len());
    if set.contains(&1) {
        ordered.push(1);
    }
    for &l in &set {
        if l != 1 && l != 2 {
            ordered.push(l);
        }
    }
    if set.contains(&2) {
        ordered.push(2);
    }
    if ordered.is_empty() {
        vec![1, 2]
    } else {
        ordered
    }
}

pub fn jlceda_layer_name(lid: i32) -> String {
    match lid {
        1 => "TopLayer".to_string(),
        2 => "BottomLayer".to_string(),
        15..=46 => format!("Inner{}", lid - 14),
        _ => format!("Layer{}", lid),
    }
}

pub fn parse_autoroute_json(
    src: &str,
    opts: &JlcedaOptions,
) -> Result<(Input, JlcedaMeta), String> {
    let ar: AutoRouteJson =
        serde_json::from_str(src).map_err(|e| format!("Invalid JLCEDA AutoRouteJson: {}", e))?;
    parse_autoroute_struct(&ar, opts)
}

pub fn parse_autoroute_struct(
    ar: &AutoRouteJson,
    opts: &JlcedaOptions,
) -> Result<(Input, JlcedaMeta), String> {
    let layers = canonical_copper_layers(&ar.layers.route);
    let nl = layers.len();
    let all_lidx: Vec<usize> = (0..nl).collect();
    let mut lid_to_idx: HashMap<i32, usize> = HashMap::new();
    for (idx, &lid) in layers.iter().enumerate() {
        lid_to_idx.insert(lid, idx);
    }

    // ---- Extract active clearance, width, and via rules
    let default_sc_key = ar
        .nets
        .first()
        .map(|n| n.safe_clearance.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("copperThickness1oz");
    let sc = ar
        .rules
        .safe_clearances
        .get(default_sc_key)
        .or_else(|| ar.rules.safe_clearances.get("copperThickness1oz"))
        .or_else(|| ar.rules.safe_clearances.values().next())
        .and_then(|v| v.first())
        .cloned()
        .unwrap_or(SafeClearanceJson {
            track_to_track: 0.102,
            track_to_via: 0.102,
            track_to_pad: 0.102,
            track_to_fill_region: 0.102,
            track_to_prohibited_region: 0.30,
            track_to_board_outline: 0.30,
            via_to_via: 0.102,
            via_to_pad: 0.102,
            via_to_prohibited_region: 0.152,
            via_to_board_outline: 0.30,
        });

    // Add safety margin so JLCEDA Pro's mil-based native DRC passes with 0 violations
    let clr_tt = (sc.track_to_track + 0.004).max(0.102);
    let clr_other = (sc.track_to_pad.max(sc.track_to_via) + 0.004)
        .max(clr_tt)
        .max(0.125);
    let edge = (sc.track_to_board_outline.max(sc.via_to_board_outline) + 0.035).max(0.335);
    let slot_track = (sc.track_to_prohibited_region + 0.035).max(0.335);
    let slot_via = (sc.via_to_prohibited_region + 0.035).max(0.19);

    let default_tw_key = ar
        .nets
        .first()
        .map(|n| n.track_width.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("copperThickness1oz");
    let tw_vec = ar
        .rules
        .track_widths
        .get(default_tw_key)
        .or_else(|| ar.rules.track_widths.get("copperThickness1oz"))
        .or_else(|| ar.rules.track_widths.values().next())
        .and_then(|v| v.first())
        .map(|r| r.track_width.clone())
        .unwrap_or_else(|| vec![0.127, 0.127, 2.54]);

    let sig_w = tw_vec.get(1).copied().unwrap_or(0.127).clamp(0.09, 0.30);
    let pwr_w = sig_w.max(0.200);
    let widths = vec![sig_w, pwr_w];

    let [via_d, via_hole_d] = ar
        .rules
        .via_sizes
        .get("viaSize")
        .or_else(|| ar.rules.via_sizes.values().next())
        .copied()
        .unwrap_or([0.50, 0.30]);

    // ---- Build outer board polygon
    let mut outer_pts: Vec<(f64, f64)> = discretize_jlceda_path(&ar.board_outline.path);
    if outer_pts.len() < 3 {
        let [x0, x1, y0, y1] = ar.board_outline.bbox;
        outer_pts = vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
    }
    dedup_ring(&mut outer_pts);
    let outer: Vec<[f64; 2]> = outer_pts.into_iter().map(|p| [p.0, p.1]).collect();

    let mut net_ids: HashMap<String, u32> = HashMap::new();
    let mut get_nid = |name: &str| -> u32 {
        if let Some(&id) = net_ids.get(name) {
            id
        } else {
            let id = net_ids.len() as u32 + 1;
            net_ids.insert(name.to_string(), id);
            id
        }
    };

    let mut items: Vec<Item> = Vec::new();
    let mut dummy_seq = 0usize;

    // ---- 1. Prohibited Regions (central cutouts + SW* NPTH bosses + MEMS mic holes + keepouts)
    for (idx_pr, pr) in ar.prohibited_regions.iter().enumerate() {
        let mut pts = discretize_jlceda_path(&pr.path);
        dedup_ring(&mut pts);
        if pts.len() < 3 {
            continue;
        }
        let lidxs: Vec<usize> = if pr.layers.is_empty() {
            all_lidx.clone()
        } else {
            pr.layers
                .iter()
                .filter_map(|lid| lid_to_idx.get(lid).copied())
                .collect()
        };
        if lidxs.is_empty() {
            continue;
        }
        let blocks_via = lidxs.len() == nl || pr.layers.contains(&12) || pr.layers.len() >= 2;
        items.push(Item {
            net: 0,
            layers: lidxs,
            kind: "keepout".to_string(),
            poly: pts.into_iter().map(|p| [p.0, p.1]).collect(),
            clr: slot_track,
            vclr: if blocks_via { slot_via } else { -1.0 },
            name: format!("__pr_{}", idx_pr),
            ..Default::default()
        });
    }

    // ---- 2. Fill Regions
    for (idx_fr, fr) in ar.fill_regions.iter().enumerate() {
        let Some(&lidx) = lid_to_idx.get(&fr.layer) else { continue };
        let mut pts = discretize_jlceda_path(&fr.path);
        dedup_ring(&mut pts);
        if pts.len() < 3 {
            continue;
        }
        let nid = if fr.net.is_empty() {
            dummy_seq += 1;
            get_nid(&format!("__fill_{}_{}", idx_fr, dummy_seq))
        } else {
            get_nid(&fr.net)
        };
        items.push(Item {
            net: nid,
            layers: vec![lidx],
            kind: "pad".to_string(),
            poly: pts.into_iter().map(|p| [p.0, p.1]).collect(),
            clr: (sc.track_to_fill_region + 0.004).max(clr_other),
            ..Default::default()
        });
    }

    // ---- 3. Components + Footprints -> Pads
    // Exact JLCEDA Pro v4.1+ transform (`jerboa/worker.js`):
    // `(isTop ? ORIGIN : MIRROR_Y).rotateD(rotation).translate(location)`
    // where `MIRROR_Y = diag(1, -1, 1)` (negates local `y` BEFORE CCW rotation!)
    for (refdes, comp) in &ar.components {
        let Some(fp) = ar.footprints.get(&comp.footprint) else { continue };
        let is_bot = comp.layer == 2;
        let rad = comp.rotation.to_radians();
        let (ca, sa) = (rad.cos(), rad.sin());
        let [cx, cy] = comp.location;

        let tf_pt = |lx: f64, ly: f64| -> (f64, f64) {
            let my = if is_bot { -ly } else { ly };
            (cx + lx * ca - my * sa, cy + lx * sa + my * ca)
        };

        for (pad_key, pad) in &fp.pads {
            let lidxs: Vec<usize> = if pad.layers.len() > 1 || pad.layers.contains(&12) {
                all_lidx.clone()
            } else {
                pad.layers
                    .iter()
                    .filter_map(|&pl| {
                        let mapped_lid = if is_bot {
                            match pl {
                                1 => 2,
                                2 => 1,
                                other => other,
                            }
                        } else {
                            pl
                        };
                        lid_to_idx.get(&mapped_lid).copied()
                    })
                    .collect()
            };
            if lidxs.is_empty() {
                continue;
            }

            let net_name = comp.nets.get(pad_key).cloned().unwrap_or_default();
            let nid = if net_name.is_empty() {
                dummy_seq += 1;
                get_nid(&format!("__nc_{}_{}_{}", refdes, pad_key, dummy_seq))
            } else {
                get_nid(&net_name)
            };

            let (pcx, pcy) = tf_pt(pad.location[0], pad.location[1]);
            let local_pts = discretize_jlceda_path(&pad.path);
            let poly: Vec<[f64; 2]> = if local_pts.len() >= 3 {
                let mut gpts: Vec<(f64, f64)> =
                    local_pts.into_iter().map(|(lx, ly)| tf_pt(lx, ly)).collect();
                dedup_ring(&mut gpts);
                if gpts.len() >= 3 {
                    gpts.into_iter().map(|p| [p.0, p.1]).collect()
                } else {
                    let r = (pad.diameter.unwrap_or(0.5) / 2.0).max(0.05);
                    circle_pts(pcx, pcy, r, 16)
                }
            } else {
                let r = (pad.diameter.unwrap_or(0.5) / 2.0).max(0.05);
                circle_pts(pcx, pcy, r, 16)
            };

            let pad_num = match &pad.number {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                _ => pad_key.clone(),
            };
            items.push(Item {
                net: nid,
                layers: lidxs,
                kind: "pad".to_string(),
                poly,
                center: [pcx, pcy],
                name: format!("{}_{}", refdes, pad_num),
                ..Default::default()
            });
        }
    }

    // ---- 4. Pre-existing Tracks & Vias (unless `opts.ignore_existing`)
    let mut preserved_wires = Vec::new();
    let mut preserved_vias = Vec::new();
    if !opts.ignore_existing {
        for tr in &ar.tracks {
            let Some(&lidx) = lid_to_idx.get(&tr.layer) else { continue };
            let nid = if tr.net.is_empty() {
                dummy_seq += 1;
                get_nid(&format!("__trk_{}", dummy_seq))
            } else {
                get_nid(&tr.net)
            };
            let w = tr.width.max(0.05);
            let tr_pts = discretize_jlceda_path(&tr.path);
            for seg in tr_pts.windows(2) {
                if (seg[0].0 - seg[1].0).hypot(seg[0].1 - seg[1].1) < 1e-5 {
                    continue;
                }
                items.push(Item {
                    net: nid,
                    layers: vec![lidx],
                    kind: "track".to_string(),
                    a: [seg[0].0, seg[0].1],
                    b: [seg[1].0, seg[1].1],
                    w,
                    ..Default::default()
                });
                if !tr.net.is_empty() {
                    preserved_wires.push(LineSeg {
                        net: tr.net.clone(),
                        layer: tr.layer,
                        x1: seg[0].0,
                        y1: seg[0].1,
                        x2: seg[1].0,
                        y2: seg[1].1,
                        width: w,
                    });
                }
            }
        }
        for v in &ar.vias {
            let nid = if v.net.is_empty() {
                dummy_seq += 1;
                get_nid(&format!("__via_{}", dummy_seq))
            } else {
                get_nid(&v.net)
            };
            let od = if v.size[0] > 0.05 { v.size[0] } else { via_d };
            let hd = if v.size[1] > 0.05 { v.size[1] } else { via_hole_d };
            items.push(Item {
                net: nid,
                layers: all_lidx.clone(),
                kind: "via".to_string(),
                c: v.location,
                r: od / 2.0,
                ..Default::default()
            });
            if !v.net.is_empty() {
                preserved_vias.push(ViaOut {
                    net: v.net.clone(),
                    x: v.location[0],
                    y: v.location[1],
                    diameter: od,
                    hole: hd,
                });
            }
        }
    }

    // ---- 5. Build routable nets & copper islands
    let mut items_by_net: HashMap<u32, Vec<usize>> = HashMap::new();
    for (idx, it) in items.iter().enumerate() {
        if it.kind != "keepout" && it.net > 0 {
            items_by_net.entry(it.net).or_default().push(idx);
        }
    }

    // Collect candidate routable nets (either listed in `ar.nets` with `routing: true`, or all non-empty component nets if `ar.nets` is empty)
    let mut candidate_nets: Vec<String> = if !ar.nets.is_empty() {
        ar.nets
            .iter()
            .filter(|n| n.routing && !n.net.is_empty())
            .map(|n| n.net.clone())
            .collect()
    } else {
        let mut s: Vec<String> = net_ids
            .keys()
            .filter(|k| !k.starts_with("__"))
            .cloned()
            .collect();
        s.sort();
        s
    };
    candidate_nets.dedup();

    let mut nets_in: Vec<NetIn> = Vec::new();
    for nname in candidate_nets {
        if opts.skip_nets.contains(&nname) {
            continue;
        }
        let Some(&nid) = net_ids.get(&nname) else { continue };
        let Some(idxs) = items_by_net.get(&nid) else { continue };
        let islands = compute_net_islands(&items, idxs, 0.004);
        if islands.len() < 2 {
            continue;
        }
        let is_pwr = opts.power_nets.contains(&nname)
            || (opts.auto_power && is_default_power_net(&nname));
        nets_in.push(NetIn {
            id: nid,
            name: nname,
            width: if is_pwr { pwr_w } else { sig_w },
            cls: if is_pwr { 1 } else { 0 },
            islands,
        });
    }

    let rules = Rules {
        clr_tt,
        clr_other,
        edge,
        slot_track,
        slot_via,
        via_d,
        via_hole_d,
        hole_clr: def_hole_clr(),
        via_cost: def_via_cost(),
        bend_cost: def_bend_cost(),
        widths,
    };

    let input = Input {
        h: opts.grid_h,
        layers: layers.clone(),
        rules,
        items,
        nets: nets_in,
        board: Board {
            outer,
            holes: Vec::new(),
        },
    };
    let meta = JlcedaMeta {
        bbox: ar.board_outline.bbox,
        layers,
        via_d,
        via_hole_d,
        preserved_wires,
        preserved_vias,
    };
    Ok((input, meta))
}

/// Export JLCEDA Pro `.ses` session file (shifting canvas mm coordinates by `(-bbox[0], -bbox[2])`
/// and mapping JLCEDA layer IDs `1, 15..18, 2` -> `TopLayer, Inner1..4, BottomLayer` so JLCEDA Pro's
/// `File -> Import -> Auto Route Session File (.ses)` places every wire & via at exact coordinates!).
pub fn write_jlceda_ses(meta: &JlcedaMeta, out: &RouteOutput, new_only: bool) -> String {
    let layer_names: Vec<String> = meta.layers.iter().map(|&l| jlceda_layer_name(l)).collect();
    let mut lid_to_seq: HashMap<i32, i32> = HashMap::new();
    for (i, &lid) in meta.layers.iter().enumerate() {
        lid_to_seq.insert(lid, (i + 1) as i32);
    }

    let remap_seg = |s: &LineSeg| -> LineSeg {
        let mut c = s.clone();
        c.layer = lid_to_seq.get(&s.layer).copied().unwrap_or(1);
        c
    };

    let dsn_meta = DsnMeta {
        pcb_name: "JLCEDA_PCB".to_string(),
        res_unit: "mil".to_string(),
        res_div: 1000.0,
        layer_names,
        via_padstack: "via0".to_string(),
        via_d_mm: meta.via_d,
        preserved_wires: meta.preserved_wires.iter().map(remap_seg).collect(),
        preserved_vias: meta.preserved_vias.clone(),
    };
    let remapped_out = RouteOutput {
        segs: out.segs.iter().map(remap_seg).collect(),
        vias: out.vias.clone(),
        fail: out.fail.clone(),
        stats: out.stats.clone(),
    };
    write_ses_string(
        &dsn_meta,
        &remapped_out,
        new_only,
        (-meta.bbox[0], -meta.bbox[2]),
    )
}
