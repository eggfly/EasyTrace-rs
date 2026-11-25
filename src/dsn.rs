//! Pure-Rust Specctra `.dsn` parser and `.ses` session writer.

use crate::model::{
    compute_net_islands, dedup_ring, def_bend_cost, def_hole_clr, def_slot_tr, def_slot_via,
    def_via_cost, is_default_power_net, poly_centroid, Board, Input, Item, LineSeg, NetIn,
    RouteOutput, Rules, ViaOut,
};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Clone, Debug)]
enum SExpr {
    Atom(String),
    List(Vec<SExpr>),
}

impl SExpr {
    fn as_atom(&self) -> Option<&str> {
        match self {
            SExpr::Atom(s) => Some(s.as_str()),
            _ => None,
        }
    }
    fn as_list(&self) -> Option<&[SExpr]> {
        match self {
            SExpr::List(v) => Some(v.as_slice()),
            _ => None,
        }
    }
    fn head(&self) -> Option<&str> {
        self.as_list()?.first()?.as_atom()
    }
}

fn tokenize_sexpr(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = src.as_bytes();
    let n = bytes.len();
    let mut i = 0;
    while i < n {
        let c = bytes[i] as char;
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        // Line comments starting with `#` when at start of line or after whitespace
        if c == '#' && (i == 0 || (bytes[i - 1] as char).is_ascii_whitespace()) {
            while i < n && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == '(' || c == ')' {
            out.push(c.to_string());
            i += 1;
            continue;
        }
        // Specctra `(string_quote ")` or `(string_quote ')` directive: single quote char atom!
        if (c == '"' || c == '\'')
            && out
                .last()
                .map(|s| s.eq_ignore_ascii_case("string_quote"))
                .unwrap_or(false)
        {
            out.push(c.to_string());
            i += 1;
            continue;
        }
        if c == '"' || c == '\'' {
            let q = c as u8;
            i += 1;
            let start = i;
            while i < n && bytes[i] != q {
                i += 1;
            }
            let s = String::from_utf8_lossy(&bytes[start..i]).to_string();
            if i < n && bytes[i] == q {
                i += 1;
            }
            out.push(s);
            continue;
        }
        let start = i;
        while i < n {
            let ch = bytes[i] as char;
            if ch.is_ascii_whitespace() || ch == '(' || ch == ')' {
                break;
            }
            i += 1;
        }
        out.push(String::from_utf8_lossy(&bytes[start..i]).to_string());
    }
    out
}

fn parse_sexpr(src: &str) -> Result<SExpr, String> {
    let toks = tokenize_sexpr(src);
    let mut stack: Vec<Vec<SExpr>> = vec![Vec::new()];
    for t in toks {
        if t == "(" {
            stack.push(Vec::new());
        } else if t == ")" {
            if stack.len() <= 1 {
                return Err("Unmatched ')' in DSN file".to_string());
            }
            let finished = stack.pop().unwrap();
            stack.last_mut().unwrap().push(SExpr::List(finished));
        } else {
            stack.last_mut().unwrap().push(SExpr::Atom(t));
        }
    }
    if stack.len() != 1 {
        return Err("Unclosed '(' in DSN file".to_string());
    }
    let mut root = stack.pop().unwrap();
    if root.is_empty() {
        return Err("Empty DSN file".to_string());
    }
    Ok(root.remove(0))
}

fn unit_to_mm(u: &str) -> f64 {
    match u.to_ascii_lowercase().as_str() {
        "mm" => 1.0,
        "um" | "micron" | "microns" => 1e-3,
        "mil" | "mils" | "thou" => 0.0254,
        "inch" | "in" | "inches" => 25.4,
        "cm" => 10.0,
        _ => 0.0254,
    }
}

#[derive(Clone, Debug)]
pub struct DsnOptions {
    pub grid_h: f64,
    pub skip_nets: HashSet<String>,
    pub power_nets: HashSet<String>,
    pub auto_power: bool,
    pub ignore_wiring: bool,
    pub ses_new_only: bool,
}

impl Default for DsnOptions {
    fn default() -> Self {
        let mut skip = HashSet::new();
        skip.insert("GND".to_string());
        Self {
            grid_h: 0.05,
            skip_nets: skip,
            power_nets: HashSet::new(),
            auto_power: true,
            ignore_wiring: false,
            ses_new_only: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct DsnMeta {
    pub pcb_name: String,
    pub res_unit: String,
    pub res_div: f64,
    pub layer_names: Vec<String>,
    pub via_padstack: String,
    pub via_d_mm: f64,
    pub preserved_wires: Vec<LineSeg>,
    pub preserved_vias: Vec<ViaOut>,
}

#[derive(Clone, Debug)]
enum PadShapeLocal {
    Circle { layer: String, diam: f64, cx: f64, cy: f64 },
    Poly { layer: String, pts: Vec<(f64, f64)> },
}

#[derive(Clone, Debug)]
struct ImagePin {
    padstack: String,
    pin_id: String,
    x: f64,
    y: f64,
    rot_deg: f64,
}

fn circle_poly(cx: f64, cy: f64, r: f64, n: usize) -> Vec<(f64, f64)> {
    (0..n)
        .map(|k| {
            let a = 2.0 * std::f64::consts::PI * (k as f64) / (n as f64);
            (cx + r * a.cos(), cy + r * a.sin())
        })
        .collect()
}

fn capsule_poly(a: (f64, f64), b: (f64, f64), r: f64) -> Vec<(f64, f64)> {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = dx.hypot(dy);
    if len < 1e-6 {
        return circle_poly(a.0, a.1, r.max(0.01), 16);
    }
    let base = dy.atan2(dx);
    let mut pts = Vec::with_capacity(18);
    for k in 0..=8 {
        let ang = base + std::f64::consts::FRAC_PI_2 + std::f64::consts::PI * (k as f64) / 8.0;
        pts.push((a.0 + r * ang.cos(), a.1 + r * ang.sin()));
    }
    for k in 0..=8 {
        let ang = base - std::f64::consts::FRAC_PI_2 + std::f64::consts::PI * (k as f64) / 8.0;
        pts.push((b.0 + r * ang.cos(), b.1 + r * ang.sin()));
    }
    pts
}

fn parse_f64_slice(items: &[SExpr], scale: f64) -> Vec<f64> {
    items
        .iter()
        .filter_map(|x| x.as_atom()?.parse::<f64>().ok().map(|v| v * scale))
        .collect()
}

fn parse_shape_node(shape_list: &[SExpr], scale: f64) -> Option<PadShapeLocal> {
    // `(shape (circle Layer D [cx cy]))` or `(circle Layer D ...)` directly
    let geom = if shape_list
        .first()
        .and_then(|x| x.as_atom())
        .map(|s| s.eq_ignore_ascii_case("shape"))
        .unwrap_or(false)
    {
        shape_list.iter().skip(1).find_map(|x| x.as_list())?
    } else {
        shape_list
    };
    let kind = geom.first()?.as_atom()?.to_ascii_lowercase();
    let layer = geom.get(1)?.as_atom()?.to_string();
    match kind.as_str() {
        "circle" => {
            let nums = parse_f64_slice(&geom[2..], scale);
            let diam = *nums.first()?;
            let cx = nums.get(1).copied().unwrap_or(0.0);
            let cy = nums.get(2).copied().unwrap_or(0.0);
            Some(PadShapeLocal::Circle {
                layer,
                diam,
                cx,
                cy,
            })
        }
        "rect" => {
            let nums = parse_f64_slice(&geom[2..], scale);
            if nums.len() < 4 {
                return None;
            }
            let (x1, y1, x2, y2) = (
                nums[0].min(nums[2]),
                nums[1].min(nums[3]),
                nums[0].max(nums[2]),
                nums[1].max(nums[3]),
            );
            Some(PadShapeLocal::Poly {
                layer,
                pts: vec![(x1, y1), (x2, y1), (x2, y2), (x1, y2)],
            })
        }
        "polygon" => {
            // `(polygon Layer aperture x0 y0 x1 y1 ...)`
            let nums = parse_f64_slice(&geom[2..], scale);
            if nums.len() < 7 {
                return None;
            }
            let coords = &nums[1..];
            let mut pts = Vec::new();
            for pair in coords.chunks_exact(2) {
                pts.push((pair[0], pair[1]));
            }
            dedup_ring(&mut pts);
            if pts.len() < 3 {
                return None;
            }
            Some(PadShapeLocal::Poly { layer, pts })
        }
        "path" => {
            // `(path Layer width x0 y0 x1 y1 ...)`
            let nums = parse_f64_slice(&geom[2..], scale);
            if nums.len() < 3 {
                return None;
            }
            let w = nums[0];
            let coords = &nums[1..];
            let mut pts_raw = Vec::new();
            for pair in coords.chunks_exact(2) {
                pts_raw.push((pair[0], pair[1]));
            }
            if pts_raw.len() == 1 {
                Some(PadShapeLocal::Circle {
                    layer,
                    diam: w.max(0.02),
                    cx: pts_raw[0].0,
                    cy: pts_raw[0].1,
                })
            } else if pts_raw.len() == 2 || w > 1e-4 {
                let p0 = pts_raw[0];
                let p1 = *pts_raw.last().unwrap();
                Some(PadShapeLocal::Poly {
                    layer,
                    pts: capsule_poly(p0, p1, (w / 2.0).max(0.01)),
                })
            } else {
                dedup_ring(&mut pts_raw);
                Some(PadShapeLocal::Poly {
                    layer,
                    pts: pts_raw,
                })
            }
        }
        _ => None,
    }
}

pub fn parse_dsn(src: &str, opts: &DsnOptions) -> Result<(Input, DsnMeta), String> {
    let root = parse_sexpr(src)?;
    let top = root.as_list().ok_or("DSN root is not a list")?;
    if !top
        .first()
        .and_then(|x| x.as_atom())
        .map(|s| s.eq_ignore_ascii_case("pcb"))
        .unwrap_or(false)
    {
        return Err("Expected `(pcb ...)` at root of DSN file".to_string());
    }
    let pcb_name = top
        .get(1)
        .and_then(|x| x.as_atom())
        .unwrap_or("board")
        .to_string();

    let mut res_unit = "mil".to_string();
    let mut res_div = 1000.0f64;
    let mut explicit_unit: Option<String> = None;

    for child in &top[2..] {
        if let Some(lst) = child.as_list() {
            if let Some(h) = child.head() {
                if h.eq_ignore_ascii_case("resolution") && lst.len() >= 3 {
                    if let Some(u) = lst[1].as_atom() {
                        res_unit = u.to_string();
                    }
                    if let Some(d) = lst[2].as_atom().and_then(|s| s.parse::<f64>().ok()) {
                        res_div = d;
                    }
                } else if h.eq_ignore_ascii_case("unit") && lst.len() >= 2 {
                    if let Some(u) = lst[1].as_atom() {
                        explicit_unit = Some(u.to_string());
                    }
                }
            }
        }
    }

    let dsn_unit = explicit_unit.as_deref().unwrap_or(&res_unit);
    let scale = unit_to_mm(dsn_unit);

    let mut layer_names: Vec<String> = Vec::new();
    let mut layer_map: HashMap<String, usize> = HashMap::new();
    let mut outer_pts: Vec<[f64; 2]> = Vec::new();
    let mut keepouts_raw: Vec<(String, PadShapeLocal)> = Vec::new();
    let mut via_padstack = String::new();
    let mut default_width = 0.127f64;
    let mut clr_default = 0.102f64;
    let mut clr_ww: Option<f64> = None;

    let parse_rule_list =
        |rlst: &[SExpr], w_out: &mut Option<f64>, c_def: &mut Option<f64>, c_ww: &mut Option<f64>| {
            for rsub in &rlst[1..] {
                if let Some(sl) = rsub.as_list() {
                    if let Some(sh) = rsub.head() {
                        if sh.eq_ignore_ascii_case("width") {
                            if let Some(v) = sl.get(1).and_then(|a| a.as_atom()).and_then(|s| s.parse::<f64>().ok()) {
                                if v > 0.0 {
                                    *w_out = Some(v * scale);
                                }
                            }
                        } else if sh.eq_ignore_ascii_case("clear") || sh.eq_ignore_ascii_case("clearance") {
                            if let Some(v) = sl.get(1).and_then(|a| a.as_atom()).and_then(|s| s.parse::<f64>().ok()) {
                                let mm = v * scale;
                                let mut is_ww = false;
                                let mut has_type = false;
                                for extra in &sl[2..] {
                                    if let Some(tl) = extra.as_list() {
                                        if extra.head().map(|x| x.eq_ignore_ascii_case("type")).unwrap_or(false) {
                                            has_type = true;
                                            if tl.iter().skip(1).any(|a| {
                                                a.as_atom()
                                                    .map(|t| t.eq_ignore_ascii_case("wire_wire"))
                                                    .unwrap_or(false)
                                            }) {
                                                is_ww = true;
                                            }
                                        }
                                    }
                                }
                                if is_ww {
                                    *c_ww = Some(mm);
                                } else if !has_type {
                                    *c_def = Some(mm);
                                } else if c_def.is_none() {
                                    *c_def = Some(mm);
                                }
                            }
                        }
                    }
                }
            }
        };

    // ---- 1. `(structure ...)`
    for child in &top[2..] {
        if child.head().map(|h| h.eq_ignore_ascii_case("structure")).unwrap_or(false) {
            for sc in &child.as_list().unwrap()[1..] {
                let Some(sl) = sc.as_list() else { continue };
                let Some(sh) = sc.head() else { continue };
                if sh.eq_ignore_ascii_case("layer") && sl.len() >= 2 {
                    let lname = sl[1].as_atom().unwrap_or("").to_string();
                    let mut is_copper = true;
                    for prop in &sl[2..] {
                        if prop.head().map(|x| x.eq_ignore_ascii_case("type")).unwrap_or(false) {
                            let t = prop
                                .as_list()
                                .and_then(|l| l.get(1))
                                .and_then(|a| a.as_atom())
                                .unwrap_or("signal");
                            is_copper = matches!(
                                t.to_ascii_lowercase().as_str(),
                                "signal" | "mixed" | "jumper"
                            );
                        }
                    }
                    if is_copper && !lname.is_empty() && !layer_map.contains_key(&lname) {
                        let idx = layer_names.len();
                        layer_names.push(lname.clone());
                        layer_map.insert(lname, idx);
                    }
                } else if sh.eq_ignore_ascii_case("boundary") {
                    for bsub in &sl[1..] {
                        if let Some(bl) = bsub.as_list() {
                            if let Some(PadShapeLocal::Poly { pts, .. }) = parse_shape_node(bl, scale) {
                                if pts.len() > outer_pts.len() {
                                    outer_pts = pts.into_iter().map(|p| [p.0, p.1]).collect();
                                }
                            }
                        }
                    }
                } else if sh.eq_ignore_ascii_case("keepout")
                    || sh.eq_ignore_ascii_case("wire_keepout")
                    || sh.eq_ignore_ascii_case("via_keepout")
                {
                    let kkind = sh.to_ascii_lowercase();
                    for ksub in &sl[1..] {
                        if let Some(kl) = ksub.as_list() {
                            if let Some(ps) = parse_shape_node(kl, scale) {
                                keepouts_raw.push((kkind.clone(), ps));
                            }
                        }
                    }
                } else if sh.eq_ignore_ascii_case("via") {
                    if via_padstack.is_empty() {
                        if let Some(vn) = sl.get(1).and_then(|a| a.as_atom()) {
                            via_padstack = vn.to_string();
                        }
                    }
                } else if sh.eq_ignore_ascii_case("rule") {
                    let (mut w_o, mut cd_o, mut cw_o) = (None, None, None);
                    parse_rule_list(sl, &mut w_o, &mut cd_o, &mut cw_o);
                    if let Some(w) = w_o {
                        default_width = w;
                    }
                    if let Some(c) = cd_o {
                        clr_default = c;
                    }
                    if let Some(cw) = cw_o {
                        clr_ww = Some(cw);
                    }
                }
            }
        }
    }

    if layer_names.is_empty() {
        for def_l in ["TopLayer", "BottomLayer"] {
            let idx = layer_names.len();
            layer_names.push(def_l.to_string());
            layer_map.insert(def_l.to_string(), idx);
        }
    }
    let nl = layer_names.len();
    let all_lidx: Vec<usize> = (0..nl).collect();

    // ---- 2. `(library ...)`
    let mut padstacks: HashMap<String, Vec<PadShapeLocal>> = HashMap::new();
    let mut images: HashMap<String, Vec<ImagePin>> = HashMap::new();

    for child in &top[2..] {
        if child.head().map(|h| h.eq_ignore_ascii_case("library")).unwrap_or(false) {
            for lc in &child.as_list().unwrap()[1..] {
                let Some(ll) = lc.as_list() else { continue };
                let Some(lh) = lc.head() else { continue };
                if lh.eq_ignore_ascii_case("padstack") && ll.len() >= 2 {
                    let ps_name = ll[1].as_atom().unwrap_or("").to_string();
                    let mut shapes = Vec::new();
                    for sub in &ll[2..] {
                        if sub.head().map(|x| x.eq_ignore_ascii_case("shape")).unwrap_or(false) {
                            if let Some(ps) = parse_shape_node(sub.as_list().unwrap(), scale) {
                                shapes.push(ps);
                            }
                        }
                    }
                    padstacks.insert(ps_name, shapes);
                } else if lh.eq_ignore_ascii_case("image") && ll.len() >= 2 {
                    let img_name = ll[1].as_atom().unwrap_or("").to_string();
                    let mut pins = Vec::new();
                    for sub in &ll[2..] {
                        if sub.head().map(|x| x.eq_ignore_ascii_case("pin")).unwrap_or(false) {
                            let pl = sub.as_list().unwrap();
                            // `(pin <padstack> [(rotate <deg>)] <pin_id> <x> <y>)`
                            let mut rot_deg = 0.0f64;
                            let mut atoms = Vec::new();
                            for elem in &pl[1..] {
                                if let Some(a) = elem.as_atom() {
                                    atoms.push(a);
                                } else if elem
                                    .head()
                                    .map(|x| x.eq_ignore_ascii_case("rotate"))
                                    .unwrap_or(false)
                                {
                                    if let Some(r) = elem
                                        .as_list()
                                        .and_then(|l| l.get(1))
                                        .and_then(|a| a.as_atom())
                                        .and_then(|s| s.parse::<f64>().ok())
                                    {
                                        rot_deg = r;
                                    }
                                }
                            }
                            if atoms.len() >= 4 {
                                let x = atoms[2].parse::<f64>().unwrap_or(0.0) * scale;
                                let y = atoms[3].parse::<f64>().unwrap_or(0.0) * scale;
                                pins.push(ImagePin {
                                    padstack: atoms[0].to_string(),
                                    pin_id: atoms[1].to_string(),
                                    x,
                                    y,
                                    rot_deg,
                                });
                            }
                        }
                    }
                    images.insert(img_name, pins);
                }
            }
        }
    }

    // Determine via OD from `via_padstack`
    let mut via_d = 0.50f64;
    if let Some(vshapes) = padstacks.get(&via_padstack) {
        for vs in vshapes {
            if let PadShapeLocal::Circle { diam, .. } = vs {
                if *diam > 0.1 {
                    via_d = (*diam * 1000.0).round() / 1000.0;
                    break;
                }
            }
        }
    } else if via_padstack.is_empty() {
        via_padstack = "via0".to_string();
    }
    let via_hole_d = if via_d <= 0.451 { 0.20 } else { 0.30 };

    // ---- 3. `(placement ...)`
    struct PlacedComp {
        image: String,
        refdes: String,
        x: f64,
        y: f64,
        back: bool,
        rot_deg: f64,
    }
    let mut placed_comps = Vec::new();
    for child in &top[2..] {
        if child.head().map(|h| h.eq_ignore_ascii_case("placement")).unwrap_or(false) {
            for pc in &child.as_list().unwrap()[1..] {
                if pc.head().map(|h| h.eq_ignore_ascii_case("component")).unwrap_or(false) {
                    let cl = pc.as_list().unwrap();
                    let img_name = cl.get(1).and_then(|a| a.as_atom()).unwrap_or("").to_string();
                    for sub in &cl[2..] {
                        if sub.head().map(|h| h.eq_ignore_ascii_case("place")).unwrap_or(false) {
                            let pl = sub.as_list().unwrap();
                            if pl.len() >= 6 {
                                let refdes = pl[1].as_atom().unwrap_or("").to_string();
                                let x = pl[2].as_atom().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0) * scale;
                                let y = pl[3].as_atom().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0) * scale;
                                let side = pl[4].as_atom().unwrap_or("front");
                                let rot_deg = pl[5].as_atom().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
                                placed_comps.push(PlacedComp {
                                    image: img_name.clone(),
                                    refdes,
                                    x,
                                    y,
                                    back: side.eq_ignore_ascii_case("back") || side.eq_ignore_ascii_case("bottom"),
                                    rot_deg,
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    // ---- 4. `(network ...)`
    let mut pin_to_net: HashMap<String, String> = HashMap::new();
    let mut net_order: Vec<String> = Vec::new();
    let mut net_width_override: HashMap<String, f64> = HashMap::new();

    for child in &top[2..] {
        if child.head().map(|h| h.eq_ignore_ascii_case("network")).unwrap_or(false) {
            for nc in &child.as_list().unwrap()[1..] {
                let Some(nlst) = nc.as_list() else { continue };
                let Some(nh) = nc.head() else { continue };
                if nh.eq_ignore_ascii_case("net") && nlst.len() >= 2 {
                    let nname = nlst[1].as_atom().unwrap_or("").to_string();
                    if !nname.is_empty() && !net_order.contains(&nname) {
                        net_order.push(nname.clone());
                    }
                    for sub in &nlst[2..] {
                        if sub.head().map(|x| x.eq_ignore_ascii_case("pins")).unwrap_or(false) {
                            for p_tok in &sub.as_list().unwrap()[1..] {
                                if let Some(pk) = p_tok.as_atom() {
                                    pin_to_net.insert(pk.to_string(), nname.clone());
                                }
                            }
                        }
                    }
                } else if nh.eq_ignore_ascii_case("class") && nlst.len() >= 2 {
                    let mut members: Vec<String> = Vec::new();
                    let (mut cw, mut cc, mut cww) = (None, None, None);
                    for sub in &nlst[2..] {
                        if let Some(a) = sub.as_atom() {
                            if !a.is_empty() {
                                members.push(a.to_string());
                            }
                        } else if sub.head().map(|x| x.eq_ignore_ascii_case("rule")).unwrap_or(false) {
                            parse_rule_list(sub.as_list().unwrap(), &mut cw, &mut cc, &mut cww);
                        }
                    }
                    if let Some(w) = cw {
                        for m in members {
                            net_width_override.insert(m, w);
                        }
                    }
                }
            }
        }
    }

    // ---- 5. `(wiring ...)` (parse before building pads so we can detect JLCEDA synthetic via-pins!)
    let mut wiring_vias_raw: Vec<(String, f64, f64)> = Vec::new();
    let mut wiring_wires_raw: Vec<(String, usize, i32, (f64, f64), (f64, f64), f64)> = Vec::new();

    for child in &top[2..] {
        if child.head().map(|h| h.eq_ignore_ascii_case("wiring")).unwrap_or(false) {
            for wc in &child.as_list().unwrap()[1..] {
                let Some(wl) = wc.as_list() else { continue };
                let Some(wh) = wc.head() else { continue };
                if wh.eq_ignore_ascii_case("via") && wl.len() >= 4 {
                    let vx = wl[2].as_atom().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0) * scale;
                    let vy = wl[3].as_atom().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0) * scale;
                    let mut vnet = String::new();
                    for sub in &wl[4..] {
                        if sub.head().map(|x| x.eq_ignore_ascii_case("net")).unwrap_or(false) {
                            vnet = sub
                                .as_list()
                                .and_then(|l| l.get(1))
                                .and_then(|a| a.as_atom())
                                .unwrap_or("")
                                .to_string();
                        }
                    }
                    wiring_vias_raw.push((vnet, vx, vy));
                } else if wh.eq_ignore_ascii_case("wire") {
                    let mut wnet = String::new();
                    let mut path_opt: Option<(usize, f64, Vec<(f64, f64)>)> = None;
                    for sub in &wl[1..] {
                        if sub.head().map(|x| x.eq_ignore_ascii_case("net")).unwrap_or(false) {
                            wnet = sub
                                .as_list()
                                .and_then(|l| l.get(1))
                                .and_then(|a| a.as_atom())
                                .unwrap_or("")
                                .to_string();
                        } else if sub
                            .head()
                            .map(|x| x.eq_ignore_ascii_case("path") || x.eq_ignore_ascii_case("polyline_path"))
                            .unwrap_or(false)
                        {
                            let pl = sub.as_list().unwrap();
                            if pl.len() >= 7 {
                                let lname = pl[1].as_atom().unwrap_or("");
                                if let Some(&lidx) = layer_map.get(lname) {
                                    let nums = parse_f64_slice(&pl[2..], scale);
                                    if nums.len() >= 5 {
                                        let ww = nums[0].max(0.05);
                                        let mut pts = Vec::new();
                                        for pair in nums[1..].chunks_exact(2) {
                                            pts.push((pair[0], pair[1]));
                                        }
                                        path_opt = Some((lidx, ww, pts));
                                    }
                                }
                            }
                        }
                    }
                    if let Some((lidx, ww, pts)) = path_opt {
                        for wpair in pts.windows(2) {
                            if (wpair[0].0 - wpair[1].0).hypot(wpair[0].1 - wpair[1].1) > 1e-5 {
                                wiring_wires_raw.push((
                                    wnet.clone(),
                                    lidx,
                                    (lidx + 1) as i32,
                                    wpair[0],
                                    wpair[1],
                                    ww,
                                ));
                            }
                        }
                    }
                }
            }
        }
    }

    // ---- Build `Item`s and `NetIn`s
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

    // Keepout regions from `(structure ...)`
    for (kkind, ps) in keepouts_raw {
        let map_layer = |ln: &str| -> Vec<usize> {
            if ln.eq_ignore_ascii_case("signal")
                || ln.eq_ignore_ascii_case("pcb")
                || ln.eq_ignore_ascii_case("all")
            {
                all_lidx.clone()
            } else if let Some(&li) = layer_map.get(ln) {
                vec![li]
            } else {
                Vec::new()
            }
        };
        let (clr, vclr) = match kkind.as_str() {
            "wire_keepout" => (clr_default.max(0.15), -1.0),
            "via_keepout" => (0.0, clr_default.max(0.15)),
            _ => (clr_default.max(0.15), clr_default.max(0.15)),
        };
        match ps {
            PadShapeLocal::Circle {
                layer,
                diam,
                cx,
                cy,
            } => {
                let lidxs = map_layer(&layer);
                if !lidxs.is_empty() && kkind != "via_keepout" {
                    items.push(Item {
                        net: 0,
                        layers: lidxs,
                        kind: "keepout".to_string(),
                        c: [cx, cy],
                        r: diam / 2.0,
                        clr,
                        vclr,
                        ..Default::default()
                    });
                } else if !lidxs.is_empty() {
                    dummy_seq += 1;
                    let tag = get_nid(&format!("__vko_{}", dummy_seq));
                    items.push(Item {
                        net: tag,
                        layers: lidxs,
                        kind: "slot".to_string(),
                        c: [cx, cy],
                        r: diam / 2.0,
                        clr: 0.0,
                        vclr,
                        ..Default::default()
                    });
                }
            }
            PadShapeLocal::Poly { layer, pts } => {
                let lidxs = map_layer(&layer);
                if !lidxs.is_empty() {
                    items.push(Item {
                        net: 0,
                        layers: lidxs,
                        kind: "keepout".to_string(),
                        poly: pts.into_iter().map(|p| [p.0, p.1]).collect(),
                        clr,
                        vclr,
                        ..Default::default()
                    });
                }
            }
        }
    }

    // Placed component pins -> pads
    for comp in &placed_comps {
        let Some(pins) = images.get(&comp.image) else { continue };
        let cread = comp.rot_deg.to_radians();
        let (cca, csa) = (cread.cos(), cread.sin());
        for pin in pins {
            let mut lx = pin.x;
            let ly = pin.y;
            if comp.back {
                lx = -lx;
            }
            let gx = comp.x + lx * cca - ly * csa;
            let gy = comp.y + lx * csa + ly * cca;

            // Check if this pin is a synthetic JLCEDA via-pin coinciding with `(wiring (via ...))`
            let is_synth_via = wiring_vias_raw
                .iter()
                .any(|(_, vx, vy)| (gx - vx).hypot(gy - vy) <= 0.004);
            if is_synth_via {
                continue;
            }

            let pin_key = format!("{}-{}", comp.refdes, pin.pin_id);
            let net_name = pin_to_net.get(&pin_key).cloned().unwrap_or_default();
            let nid = if net_name.is_empty() {
                dummy_seq += 1;
                get_nid(&format!("__unc_{}", dummy_seq))
            } else {
                get_nid(&net_name)
            };

            let Some(pshapes) = padstacks.get(&pin.padstack) else { continue };
            let total_rot = if comp.back {
                (comp.rot_deg - pin.rot_deg).to_radians()
            } else {
                (comp.rot_deg + pin.rot_deg).to_radians()
            };
            let (pca, psa) = (total_rot.cos(), total_rot.sin());
            let map_placed_layer = |lname: &str| -> Option<usize> {
                let li = *layer_map.get(lname)?;
                if comp.back {
                    if li == 0 && nl >= 2 {
                        Some(nl - 1)
                    } else if li == nl - 1 && nl >= 2 {
                        Some(0)
                    } else {
                        Some(li)
                    }
                } else {
                    Some(li)
                }
            };

            // Group identical shapes across layers (e.g. through-hole pads spanning all 6 layers)
            let mut by_geom: Vec<(Vec<usize>, Vec<[f64; 2]>, [f64; 2])> = Vec::new();
            for ps in pshapes {
                match ps {
                    PadShapeLocal::Circle {
                        layer,
                        diam,
                        cx,
                        cy,
                    } => {
                        let Some(li) = map_placed_layer(layer) else { continue };
                        let mut sx = *cx;
                        let sy = *cy;
                        if comp.back {
                            sx = -sx;
                        }
                        let scx = gx + sx * pca - sy * psa;
                        let scy = gy + sx * psa + sy * pca;
                        let poly: Vec<[f64; 2]> = circle_poly(scx, scy, (*diam / 2.0).max(0.01), 16)
                            .into_iter()
                            .map(|p| [p.0, p.1])
                            .collect();
                        if let Some(existing) = by_geom.iter_mut().find(|g| {
                            (g.2[0] - scx).abs() < 1e-5
                                && (g.2[1] - scy).abs() < 1e-5
                                && g.1.len() == poly.len()
                        }) {
                            if !existing.0.contains(&li) {
                                existing.0.push(li);
                            }
                        } else {
                            by_geom.push((vec![li], poly, [scx, scy]));
                        }
                    }
                    PadShapeLocal::Poly { layer, pts } => {
                        let Some(li) = map_placed_layer(layer) else { continue };
                        let mut gpts: Vec<(f64, f64)> = pts
                            .iter()
                            .map(|&(px, py)| {
                                let mx = if comp.back { -px } else { px };
                                (gx + mx * pca - py * psa, gy + mx * psa + py * pca)
                            })
                            .collect();
                        dedup_ring(&mut gpts);
                        let c = poly_centroid(&gpts);
                        let poly: Vec<[f64; 2]> = gpts.into_iter().map(|p| [p.0, p.1]).collect();
                        if let Some(existing) = by_geom.iter_mut().find(|g| {
                            (g.2[0] - c.0).abs() < 1e-5
                                && (g.2[1] - c.1).abs() < 1e-5
                                && g.1.len() == poly.len()
                        }) {
                            if !existing.0.contains(&li) {
                                existing.0.push(li);
                            }
                        } else {
                            by_geom.push((vec![li], poly, [c.0, c.1]));
                        }
                    }
                }
            }

            for (mut lidxs, poly, center) in by_geom {
                lidxs.sort_unstable();
                lidxs.dedup();
                // Unconnected all-layer padstacks in JLCEDA DSN (cutouts / NPTH bosses) get keepout/slot rules
                let is_unconn_mech = net_name.is_empty() && lidxs.len() == nl && nl >= 2;
                items.push(Item {
                    net: nid,
                    layers: lidxs,
                    kind: if is_unconn_mech {
                        "keepout".to_string()
                    } else {
                        "pad".to_string()
                    },
                    poly,
                    center,
                    clr: if is_unconn_mech { 0.30 } else { 0.0 },
                    vclr: if is_unconn_mech { 0.185 } else { 0.0 },
                    name: pin_key.clone(),
                    ..Default::default()
                });
            }
        }
    }

    let mut preserved_wires = Vec::new();
    let mut preserved_vias = Vec::new();
    if !opts.ignore_wiring {
        for (wnet, lidx, lid_num, a, b, ww) in &wiring_wires_raw {
            let nid = if wnet.is_empty() {
                dummy_seq += 1;
                get_nid(&format!("__wire_{}", dummy_seq))
            } else {
                get_nid(wnet)
            };
            items.push(Item {
                net: nid,
                layers: vec![*lidx],
                kind: "track".to_string(),
                a: [a.0, a.1],
                b: [b.0, b.1],
                w: *ww,
                ..Default::default()
            });
            if !wnet.is_empty() {
                preserved_wires.push(LineSeg {
                    net: wnet.clone(),
                    layer: *lid_num,
                    x1: a.0,
                    y1: a.1,
                    x2: b.0,
                    y2: b.1,
                    width: *ww,
                });
            }
        }
        for (vnet, vx, vy) in &wiring_vias_raw {
            let nid = if vnet.is_empty() {
                dummy_seq += 1;
                get_nid(&format!("__via_{}", dummy_seq))
            } else {
                get_nid(vnet)
            };
            items.push(Item {
                net: nid,
                layers: all_lidx.clone(),
                kind: "via".to_string(),
                c: [*vx, *vy],
                r: via_d / 2.0,
                ..Default::default()
            });
            if !vnet.is_empty() {
                preserved_vias.push(ViaOut {
                    net: vnet.clone(),
                    x: *vx,
                    y: *vy,
                    diameter: via_d,
                    hole: via_hole_d,
                });
            }
        }
    }

    // ---- Build `NetIn` islands per routable net
    let mut items_by_net: HashMap<u32, Vec<usize>> = HashMap::new();
    for (idx, it) in items.iter().enumerate() {
        if it.kind != "keepout" && it.net > 0 {
            items_by_net.entry(it.net).or_default().push(idx);
        }
    }

    let sig_w = default_width.clamp(0.09, 0.30);
    let pwr_w = sig_w.max(0.200);
    let widths = vec![sig_w, pwr_w];

    let mut nets_in: Vec<NetIn> = Vec::new();
    for nname in &net_order {
        // Skip `GND` or `GND~0` style pre-split planes
        if opts.skip_nets.contains(nname) || nname.contains('~') {
            continue;
        }
        let Some(&nid) = net_ids.get(nname) else { continue };
        let Some(idxs) = items_by_net.get(&nid) else { continue };
        let islands = compute_net_islands(&items, idxs, 0.005);
        if islands.len() < 2 {
            continue;
        }
        let nw_opt = net_width_override.get(nname).copied();
        let is_pwr = opts.power_nets.contains(nname)
            || (opts.auto_power && is_default_power_net(nname))
            || nw_opt.map(|w| w >= 0.18).unwrap_or(false);
        nets_in.push(NetIn {
            id: nid,
            name: nname.clone(),
            width: if is_pwr { pwr_w } else { sig_w },
            cls: if is_pwr { 1 } else { 0 },
            islands,
        });
    }

    let clr_tt = clr_ww.unwrap_or(clr_default).max(0.09);
    let clr_other = clr_default.max(clr_tt).max(0.125);
    let rules = Rules {
        clr_tt,
        clr_other,
        edge: 0.335,
        slot_track: def_slot_tr(),
        slot_via: def_slot_via(),
        via_d,
        via_hole_d,
        hole_clr: def_hole_clr(),
        via_cost: def_via_cost(),
        bend_cost: def_bend_cost(),
        widths,
    };

    let layers: Vec<i32> = (1..=(nl as i32)).collect();
    let input = Input {
        h: opts.grid_h,
        layers,
        rules,
        items,
        nets: nets_in,
        board: Board {
            outer: outer_pts,
            holes: Vec::new(),
        },
    };
    let meta = DsnMeta {
        pcb_name,
        res_unit,
        res_div,
        layer_names,
        via_padstack,
        via_d_mm: via_d,
        preserved_wires,
        preserved_vias,
    };
    Ok((input, meta))
}

fn quote_ses_ident(s: &str) -> String {
    if s.is_empty()
        || s.chars()
            .any(|c| c.is_ascii_whitespace() || matches!(c, '(' | ')' | '"' | '\'' | '-' | '+'))
    {
        format!("\"{}\"", s.replace('"', ""))
    } else {
        s.to_string()
    }
}

pub fn write_ses_string(meta: &DsnMeta, out: &RouteOutput, new_only: bool, shift_xy: (f64, f64)) -> String {
    let unit_mm = unit_to_mm(&meta.res_unit);
    let to_ses = |mm: f64| -> i64 { (((mm / unit_mm) * meta.res_div).round()) as i64 };
    let via_diam_ses = to_ses(meta.via_d_mm);

    // Group wires and vias per net
    let mut net_wires: BTreeMap<String, Vec<&LineSeg>> = BTreeMap::new();
    let mut net_vias: BTreeMap<String, Vec<&ViaOut>> = BTreeMap::new();

    if !new_only {
        for w in &meta.preserved_wires {
            net_wires.entry(w.net.clone()).or_default().push(w);
        }
        for v in &meta.preserved_vias {
            net_vias.entry(v.net.clone()).or_default().push(v);
        }
    }
    for w in &out.segs {
        net_wires.entry(w.net.clone()).or_default().push(w);
    }
    for v in &out.vias {
        net_vias.entry(v.net.clone()).or_default().push(v);
    }

    let mut all_nets: Vec<String> = net_wires
        .keys()
        .chain(net_vias.keys())
        .cloned()
        .collect();
    all_nets.sort();
    all_nets.dedup();

    let pcb_q = quote_ses_ident(&meta.pcb_name);
    let via_q = quote_ses_ident(&meta.via_padstack);
    let res_div_int = meta.res_div.round() as i64;

    let mut s = String::with_capacity(65536);
    s.push_str(&format!("(session {}\n", pcb_q));
    s.push_str(&format!("  (base_design {})\n", pcb_q));
    s.push_str(&format!(
        "  (placement\n    (resolution {} {})\n  )\n",
        meta.res_unit, res_div_int
    ));
    s.push_str("  (was_is\n  )\n");
    s.push_str("  (routes\n");
    s.push_str(&format!("    (resolution {} {})\n", meta.res_unit, res_div_int));
    s.push_str("    (parser\n      (host_cad \"EasyTrace-rs\")\n      (host_version \"0.2.0\")\n    )\n");
    s.push_str("    (library_out\n");
    s.push_str(&format!("      (padstack {}\n", via_q));
    for lname in &meta.layer_names {
        s.push_str(&format!(
            "        (shape (circle {} {} 0 0))\n",
            quote_ses_ident(lname),
            via_diam_ses
        ));
    }
    s.push_str("      )\n    )\n");
    s.push_str("    (network_out\n");

    for nname in all_nets {
        s.push_str(&format!("      (net {}\n", quote_ses_ident(&nname)));
        if let Some(vs) = net_vias.get(&nname) {
            let mut seen = Vec::new();
            for v in vs {
                let sx = to_ses(v.x + shift_xy.0);
                let sy = to_ses(v.y + shift_xy.1);
                if !seen.contains(&(sx, sy)) {
                    seen.push((sx, sy));
                    s.push_str(&format!("        (via {} {} {})\n", via_q, sx, sy));
                }
            }
        }
        if let Some(ws) = net_wires.get(&nname) {
            for w in ws {
                let lidx = (w.layer - 1).max(0) as usize;
                let lname = meta
                    .layer_names
                    .get(lidx)
                    .map(|x| x.as_str())
                    .unwrap_or("TopLayer");
                let sw = to_ses(w.width).max(1);
                let x1 = to_ses(w.x1 + shift_xy.0);
                let y1 = to_ses(w.y1 + shift_xy.1);
                let x2 = to_ses(w.x2 + shift_xy.0);
                let y2 = to_ses(w.y2 + shift_xy.1);
                if x1 == x2 && y1 == y2 {
                    continue;
                }
                s.push_str(&format!(
                    "        (wire\n          (path {} {} {} {} {} {})\n        )\n",
                    quote_ses_ident(lname),
                    sw,
                    x1,
                    y1,
                    x2,
                    y2
                ));
            }
        }
        s.push_str("      )\n");
    }

    s.push_str("    )\n  )\n)\n");
    s
}
