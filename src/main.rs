//! `easytrace` CLI entrypoint supporting 4 pure-Rust subcommands:
//!   1. `easytrace dsn <input.dsn> -o <output.ses> [options]`
//!   2. `easytrace jlceda <autoroute.json> -o <routes.json> [--ses <out.ses>] [options]`
//!   3. `easytrace route <geom.json> <routes.json> [options]` (and legacy positional `easytrace <geom.json> <routes.json>`)
//!   4. `easytrace serve [--port 3789] [--host 127.0.0.1]`

use easytrace_rs::dsn::{parse_dsn, write_ses_string, DsnOptions};
use easytrace_rs::engine::route_input;
use easytrace_rs::jlceda::{parse_autoroute_json, write_jlceda_ses, JlcedaOptions};
use easytrace_rs::model::{Input, RouteConfig};
use easytrace_rs::server::run_server;
use std::collections::HashSet;

fn print_help() {
    eprintln!(
        "\
EasyTrace-rs v{ver} — Pure-Rust PathFinder Negotiated-Congestion PCB Autorouter

USAGE:
  easytrace dsn <input.dsn> -o <output.ses> [OPTIONS]
  easytrace jlceda <autoroute.json> -o <routes.json> [--ses <out.ses>] [OPTIONS]
  easytrace route <geom.json> <routes.json> [OPTIONS]
  easytrace <geom.json> <routes.json> [OPTIONS]
  easytrace serve [--port 3789] [--host 127.0.0.1]

SUBCOMMANDS:
  dsn      Route a Specctra .dsn file (JLCEDA Pro, KiCad, Altium) and emit a .ses session file
  jlceda   Route JLCEDA Pro's native getAutoRouteJsonFile() export (preserving cutouts & NPTH slots!)
  route    Route native EasyTrace geometry JSON (<geom.json> -> <routes.json>)
  serve    Start local HTTP server on 127.0.0.1:3789 for the JLCEDA Pro One-Click Extension

COMMON ROUTING OPTIONS:
  -o, --output <FILE>       Output file path (.ses or routes.json)
  --ses <FILE>              (jlceda mode) Also write a JLCEDA-compatible Specctra .ses file
  --algo <pf|lee|solo>      Routing algorithm [default: pf]
  --iters <N>               Max PathFinder rip-up & reroute iterations [default: 45]
  --budget <SEC>            Wall-clock time budget in seconds [default: 3600]
  --h <MM>                  Routing grid step in mm [default: 0.05]
  --window <MM>             Initial A* bounding-box margin in mm [default: 3.5]
  --skip <NET1,NET2>        Comma-separated nets to skip (e.g. poured planes) [default: GND]
  --power <NET1,NET2>       Comma-separated wide power nets (0.20mm w/ auto 0.127mm pad neckdown)
  --no-auto-power           Disable automatic power-net name detection (VBAT/VDD*/VCC*/VBUS*)
  --ignore-existing         (jlceda/dsn) Ignore pre-existing tracks & vias and route from scratch
  --ignore-wiring           Alias for --ignore-existing in dsn mode
  --new-only                (.ses output) Emit only newly routed wires & vias
  --min-width <MM>          Optional pinch-throat squeeze width (e.g. 0.095; default keeps >=0.127mm)
  --no-smooth               Disable octilinear (0/45/90 deg) pull-tight post-smoothing
  --nofull                  Restrict A* search strictly to --window bounding box
  --max-exp <N>             Per-join A* node expansion cap during rip-up [default: unlimited]
  --max-exp-hard <N>        Per-join A* node expansion cap in final hard pass [default: 1800000]
  --pres0 <F>               Initial congestion pressure multiplier [default: 0.5]
  --pres-mul <F>            Per-iteration pressure growth factor (capped at 28.0) [default: 1.35]
  --hist <F>                Historical congestion increment [default: 1.0]
  --block <L0,L1>           Comma-separated 0-based layer indices to block completely
  --plane <NET:L_IDX>       Treat 0-based layer index as full plane for NET (e.g. GND:1)
  -q, --quiet               Suppress progress/stderr logging
  -h, --help                Print help information
  -V, --version             Print version
",
        ver = env!("CARGO_PKG_VERSION")
    );
}

fn find_flag_val(args: &[String], keys: &[&str]) -> Option<String> {
    for (i, a) in args.iter().enumerate() {
        if keys.iter().any(|k| a == k) && i + 1 < args.len() {
            return Some(args[i + 1].clone());
        }
    }
    None
}

fn has_flag(args: &[String], keys: &[&str]) -> bool {
    args.iter().any(|a| keys.iter().any(|k| a == k))
}

fn parse_route_config(args: &[String]) -> RouteConfig {
    let mut cfg = RouteConfig::default();
    if let Some(v) = find_flag_val(args, &["--algo"]) {
        cfg.algo = v;
    }
    if let Some(v) = find_flag_val(args, &["--iters"]).and_then(|s| s.parse().ok()) {
        cfg.iters = v;
    }
    if let Some(v) = find_flag_val(args, &["--budget"]).and_then(|s| s.parse().ok()) {
        cfg.budget_s = v;
    }
    if let Some(v) = find_flag_val(args, &["--window"]).and_then(|s| s.parse().ok()) {
        cfg.window_mm = v;
    }
    if let Some(v) = find_flag_val(args, &["--pres0"]).and_then(|s| s.parse().ok()) {
        cfg.pres0 = v;
    }
    if let Some(v) = find_flag_val(args, &["--pres-mul"]).and_then(|s| s.parse().ok()) {
        cfg.pres_mul = v;
    }
    if let Some(v) = find_flag_val(args, &["--hist"]).and_then(|s| s.parse().ok()) {
        cfg.hist_inc = v;
    }
    if let Some(v) = find_flag_val(args, &["--min-width", "--squeeze-width"]).and_then(|s| s.parse().ok()) {
        cfg.min_width = Some(v);
    }
    if has_flag(args, &["--no-squeeze"]) {
        cfg.min_width = None;
    }
    if let Some(v) = find_flag_val(args, &["--max-exp"]).and_then(|s| s.parse().ok()) {
        cfg.max_exp = v;
    }
    if let Some(v) = find_flag_val(args, &["--max-exp-hard"]).and_then(|s| s.parse().ok()) {
        cfg.max_exp_hard = v;
    }
    cfg.no_full = has_flag(args, &["--nofull", "--no-full"]);
    cfg.no_smooth = has_flag(args, &["--no-smooth"]);
    cfg.quiet = has_flag(args, &["-q", "--quiet"]);

    if let Some(bl) = find_flag_val(args, &["--block"]) {
        cfg.block_layers = bl
            .split(',')
            .filter(|x| !x.is_empty())
            .filter_map(|x| x.parse::<usize>().ok())
            .collect();
    }
    if let Some(pl) = find_flag_val(args, &["--plane"]) {
        for item in pl.split(',').filter(|x| !x.is_empty()) {
            if let Some((n, l)) = item.split_once(':') {
                if let Ok(li) = l.parse::<usize>() {
                    cfg.planes.insert(n.to_string(), li);
                }
            }
        }
    }
    cfg
}

fn positional_args<'a>(args: &'a [String], start_idx: usize) -> Vec<&'a str> {
    let valued_flags = [
        "-o",
        "--output",
        "--ses",
        "--algo",
        "--iters",
        "--threads",
        "--budget",
        "--h",
        "--window",
        "--skip",
        "--power",
        "--min-width",
        "--squeeze-width",
        "--max-exp",
        "--max-exp-hard",
        "--pres0",
        "--pres-mul",
        "--hist",
        "--block",
        "--plane",
        "--port",
        "--host",
    ];
    let mut pos = Vec::new();
    let mut i = start_idx;
    while i < args.len() {
        let a = args[i].as_str();
        if valued_flags.contains(&a) {
            i += 2;
            continue;
        }
        if a.starts_with('-') {
            i += 1;
            continue;
        }
        pos.push(a);
        i += 1;
    }
    pos
}

fn parse_csv_set(val: Option<String>, default_items: &[&str]) -> HashSet<String> {
    match val {
        Some(s) => s
            .split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect(),
        None => default_items.iter().map(|s| (*s).to_string()).collect(),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 || has_flag(&args, &["-h", "--help", "help"]) {
        print_help();
        return;
    }
    if has_flag(&args, &["-V", "--version", "version"]) {
        println!("easytrace {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    let cmd = args[1].as_str();
    match cmd {
        "serve" => {
            let host = find_flag_val(&args, &["--host"]).unwrap_or_else(|| "127.0.0.1".to_string());
            let port: u16 = find_flag_val(&args, &["--port", "-p"])
                .and_then(|s| s.parse().ok())
                .unwrap_or(3789);
            if let Err(e) = run_server(&host, port) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        "dsn" => {
            let pos = positional_args(&args, 2);
            let Some(&in_path) = pos.first() else {
                eprintln!("Usage: easytrace dsn <input.dsn> -o <output.ses> [options]");
                std::process::exit(1);
            };
            let out_path = find_flag_val(&args, &["-o", "--output"])
                .or_else(|| pos.get(1).map(|s| (*s).to_string()))
                .unwrap_or_else(|| {
                    if let Some(stem) = in_path.strip_suffix(".dsn") {
                        format!("{}.ses", stem)
                    } else {
                        format!("{}.ses", in_path)
                    }
                });

            let cfg = parse_route_config(&args);
            let dopts = DsnOptions {
                grid_h: find_flag_val(&args, &["--h"])
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.05),
                skip_nets: parse_csv_set(find_flag_val(&args, &["--skip"]), &["GND"]),
                power_nets: parse_csv_set(find_flag_val(&args, &["--power"]), &[]),
                auto_power: !has_flag(&args, &["--no-auto-power"]),
                ignore_wiring: has_flag(&args, &["--ignore-wiring", "--ignore-existing"]),
                ses_new_only: has_flag(&args, &["--new-only"]),
            };

            let src = std::fs::read_to_string(in_path).unwrap_or_else(|e| {
                eprintln!("Failed to read {}: {}", in_path, e);
                std::process::exit(1);
            });
            let (inp, meta) = parse_dsn(&src, &dopts).unwrap_or_else(|e| {
                eprintln!("Failed to parse DSN {}: {}", in_path, e);
                std::process::exit(1);
            });
            let out = route_input(inp, &cfg);
            let ses = write_ses_string(&meta, &out, dopts.ses_new_only, (0.0, 0.0));
            std::fs::write(&out_path, ses).unwrap_or_else(|e| {
                eprintln!("Failed to write {}: {}", out_path, e);
                std::process::exit(1);
            });
            if !cfg.quiet {
                eprintln!("wrote Specctra session -> {}", out_path);
            }
        }
        "jlceda" => {
            let pos = positional_args(&args, 2);
            let Some(&in_path) = pos.first() else {
                eprintln!(
                    "Usage: easytrace jlceda <autoroute.json> -o <routes.json> [--ses <out.ses>] [options]"
                );
                std::process::exit(1);
            };
            let out_path = find_flag_val(&args, &["-o", "--output"])
                .or_else(|| pos.get(1).map(|s| (*s).to_string()))
                .unwrap_or_else(|| "routes.json".to_string());
            let ses_path = find_flag_val(&args, &["--ses"]);

            let cfg = parse_route_config(&args);
            let jopts = JlcedaOptions {
                grid_h: find_flag_val(&args, &["--h"])
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.05),
                skip_nets: parse_csv_set(find_flag_val(&args, &["--skip"]), &["GND"]),
                power_nets: parse_csv_set(find_flag_val(&args, &["--power"]), &[]),
                auto_power: !has_flag(&args, &["--no-auto-power"]),
                ignore_existing: has_flag(&args, &["--ignore-existing", "--ignore-wiring"]),
            };

            let src = std::fs::read_to_string(in_path).unwrap_or_else(|e| {
                eprintln!("Failed to read {}: {}", in_path, e);
                std::process::exit(1);
            });
            let (inp, meta) = parse_autoroute_json(&src, &jopts).unwrap_or_else(|e| {
                eprintln!("Failed to parse JLCEDA AutoRouteJson {}: {}", in_path, e);
                std::process::exit(1);
            });
            let out = route_input(inp, &cfg);
            let rjson = serde_json::to_string(&out.to_routes_json()).unwrap();
            std::fs::write(&out_path, rjson).unwrap_or_else(|e| {
                eprintln!("Failed to write {}: {}", out_path, e);
                std::process::exit(1);
            });
            if !cfg.quiet {
                eprintln!("wrote JLCEDA routes -> {}", out_path);
            }
            if let Some(sp) = ses_path {
                let ses = write_jlceda_ses(&meta, &out, has_flag(&args, &["--new-only"]));
                std::fs::write(&sp, ses).unwrap_or_else(|e| {
                    eprintln!("Failed to write {}: {}", sp, e);
                    std::process::exit(1);
                });
                if !cfg.quiet {
                    eprintln!("wrote JLCEDA Specctra session -> {}", sp);
                }
            }
        }
        "route" | _ => {
            let start = if cmd == "route" { 2 } else { 1 };
            let pos = positional_args(&args, start);
            let Some(&in_path) = pos.first() else {
                print_help();
                std::process::exit(1);
            };
            let out_path = find_flag_val(&args, &["-o", "--output"])
                .or_else(|| pos.get(1).map(|s| (*s).to_string()))
                .unwrap_or_else(|| "routes.json".to_string());

            let cfg = parse_route_config(&args);
            let src = std::fs::read_to_string(in_path).unwrap_or_else(|e| {
                eprintln!("Failed to read {}: {}", in_path, e);
                std::process::exit(1);
            });
            let inp: Input = serde_json::from_str(&src).unwrap_or_else(|e| {
                eprintln!("Failed to parse geometry JSON {}: {}", in_path, e);
                std::process::exit(1);
            });
            let out = route_input(inp, &cfg);
            let rjson = serde_json::to_string(&out.to_routes_json()).unwrap();
            std::fs::write(&out_path, rjson).unwrap_or_else(|e| {
                eprintln!("Failed to write {}: {}", out_path, e);
                std::process::exit(1);
            });
        }
    }
}
