//! Lightweight pure-Rust (`std::net::TcpListener`, zero heavy async dependencies) local HTTP server
//! powering the JLCEDA Pro One-Click Local Extension (`http://127.0.0.1:3789`).

use crate::dsn::{parse_dsn, write_ses_string, DsnOptions};
use crate::engine::route_input;
use crate::jlceda::{parse_autoroute_struct, AutoRouteJson, JlcedaOptions};
use crate::model::{Input, RouteConfig};
use serde::Deserialize;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct JlcedaRouteRequestOpts {
    iters: Option<usize>,
    budget_s: Option<f64>,
    h: Option<f64>,
    skip: Option<Vec<String>>,
    power: Option<Vec<String>>,
    ignore_existing: Option<bool>,
    no_smooth: Option<bool>,
    min_width: Option<f64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WrappedJlcedaBody {
    auto_route: AutoRouteJson,
    #[serde(default)]
    options: JlcedaRouteRequestOpts,
}

fn parse_query(path: &str) -> (&str, HashMap<String, String>) {
    let Some((base, qs)) = path.split_once('?') else {
        return (path, HashMap::new());
    };
    let mut m = HashMap::new();
    for kv in qs.split('&') {
        if kv.is_empty() {
            continue;
        }
        if let Some((k, v)) = kv.split_once('=') {
            m.insert(k.to_string(), url_decode(v));
        } else {
            m.insert(kv.to_string(), "1".to_string());
        }
    }
    (base, m)
}

fn url_decode(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(hex) = std::str::from_utf8(&b[i + 1..i + 3]) {
                if let Ok(val) = u8::from_str_radix(hex, 16) {
                    out.push(val);
                    i += 3;
                    continue;
                }
            }
        } else if b[i] == b'+' {
            out.push(b' ');
            i += 1;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn write_http_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    extra_headers: &[(&str, String)],
    body: &[u8],
) {
    let mut hdr = format!(
        "HTTP/1.1 {}\r\n\
         Content-Type: {}\r\n\
         Content-Length: {}\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n\
         Access-Control-Allow-Headers: Content-Type, X-EasyTrace-Options\r\n\
         Access-Control-Allow-Private-Network: true\r\n\
         Access-Control-Expose-Headers: X-EasyTrace-Routed-Conns, X-EasyTrace-Total-Conns\r\n\
         Connection: close\r\n",
        status,
        content_type,
        body.len()
    );
    for (k, v) in extra_headers {
        hdr.push_str(&format!("{}: {}\r\n", k, v));
    }
    hdr.push_str("\r\n");
    let _ = stream.write_all(hdr.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

fn handle_client(mut stream: TcpStream) {
    let Ok(peer) = stream.peer_addr() else { return };
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });

    let mut req_line = String::new();
    if reader.read_line(&mut req_line).is_err() || req_line.trim().is_empty() {
        return;
    }
    let parts: Vec<&str> = req_line.split_whitespace().collect();
    if parts.len() < 2 {
        return;
    }
    let method = parts[0].to_ascii_uppercase();
    let raw_target = parts[1];
    let (route_path, qparams) = parse_query(raw_target);

    let mut content_len = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        if let Some((hk, hv)) = trimmed.split_once(':') {
            if hk.trim().eq_ignore_ascii_case("content-length") {
                content_len = hv.trim().parse::<usize>().unwrap_or(0);
            }
        }
    }

    if method == "OPTIONS" {
        write_http_response(&mut stream, "204 No Content", "text/plain", &[], b"");
        return;
    }

    let mut body = vec![0u8; content_len];
    if content_len > 0 && reader.read_exact(&mut body).is_err() {
        write_http_response(
            &mut stream,
            "400 Bad Request",
            "application/json",
            &[],
            br#"{"ok":false,"error":"Failed to read full request body"}"#,
        );
        return;
    }

    match (method.as_str(), route_path) {
        ("GET", "/") | ("GET", "/health") => {
            let resp = serde_json::json!({
                "ok": true,
                "status": "ok",
                "service": "easytrace-rs",
                "version": env!("CARGO_PKG_VERSION"),
            });
            let bytes = serde_json::to_vec(&resp).unwrap();
            write_http_response(&mut stream, "200 OK", "application/json", &[], &bytes);
        }
        ("POST", "/route-jlceda") => {
            eprintln!("[serve] POST /route-jlceda from {} ({} bytes)", peer, body.len());
            let body_str = String::from_utf8_lossy(&body);

            // Support either `{ "autoRoute": {...}, "options": {...} }` OR raw `AutoRouteJson`
            let (ar, req_opts) = if let Ok(wrapped) = serde_json::from_str::<WrappedJlcedaBody>(&body_str) {
                (wrapped.auto_route, wrapped.options)
            } else {
                match serde_json::from_str::<AutoRouteJson>(&body_str) {
                    Ok(raw_ar) => (raw_ar, JlcedaRouteRequestOpts::default()),
                    Err(e) => {
                        let err = serde_json::json!({"ok": false, "error": format!("JSON parse error: {}", e)});
                        write_http_response(
                            &mut stream,
                            "400 Bad Request",
                            "application/json",
                            &[],
                            &serde_json::to_vec(&err).unwrap(),
                        );
                        return;
                    }
                }
            };

            let mut jopts = JlcedaOptions::default();
            if let Some(h) = req_opts
                .h
                .or_else(|| qparams.get("h").and_then(|s| s.parse().ok()))
            {
                jopts.grid_h = h;
            }
            if let Some(sk) = req_opts.skip {
                jopts.skip_nets = sk.into_iter().filter(|s| !s.is_empty()).collect();
            } else if let Some(sk) = qparams.get("skip") {
                jopts.skip_nets = sk
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            if let Some(pw) = req_opts.power {
                jopts.power_nets = pw.into_iter().filter(|s| !s.is_empty()).collect();
            } else if let Some(pw) = qparams.get("power") {
                jopts.power_nets = pw
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            if let Some(ig) = req_opts.ignore_existing.or_else(|| {
                qparams
                    .get("mode")
                    .map(|m| m.eq_ignore_ascii_case("replace"))
            }) {
                jopts.ignore_existing = ig;
            }

            let mut rcfg = RouteConfig::default();
            if let Some(it) = req_opts
                .iters
                .or_else(|| qparams.get("iters").and_then(|s| s.parse().ok()))
            {
                rcfg.iters = it;
            }
            if let Some(b) = req_opts
                .budget_s
                .or_else(|| qparams.get("budget").and_then(|s| s.parse().ok()))
            {
                rcfg.budget_s = b;
            }
            if let Some(ns) = req_opts.no_smooth {
                rcfg.no_smooth = ns;
            }
            if let Some(mw) = req_opts
                .min_width
                .or_else(|| qparams.get("min_width").and_then(|s| s.parse().ok()))
            {
                rcfg.min_width = Some(mw);
            }

            match parse_autoroute_struct(&ar, &jopts) {
                Ok((inp, meta)) => {
                    let out = route_input(inp, &rcfg);
                    let mut resp = out.to_routes_json();
                    resp["ok"] = serde_json::json!(true);
                    resp["viaHole"] = serde_json::json!(meta.via_hole_d);
                    resp["viaDiameter"] = serde_json::json!(meta.via_d);
                    eprintln!(
                        "[serve] Routed {}/{} conns ({}/{} nets), {} segments, {} vias in {:.2}s",
                        out.stats.conns_routed,
                        out.stats.conns_total,
                        out.stats.nets_routed,
                        out.stats.nets_total,
                        out.stats.segments,
                        out.stats.vias,
                        out.stats.elapsed_s
                    );
                    let bytes = serde_json::to_vec(&resp).unwrap();
                    write_http_response(&mut stream, "200 OK", "application/json", &[], &bytes);
                }
                Err(e) => {
                    let err = serde_json::json!({"ok": false, "error": e});
                    write_http_response(
                        &mut stream,
                        "400 Bad Request",
                        "application/json",
                        &[],
                        &serde_json::to_vec(&err).unwrap(),
                    );
                }
            }
        }
        ("POST", "/route-dsn") => {
            eprintln!("[serve] POST /route-dsn from {} ({} bytes)", peer, body.len());
            let dsn_text = String::from_utf8_lossy(&body);
            let mut dopts = DsnOptions::default();
            if let Some(sk) = qparams.get("skip") {
                dopts.skip_nets = sk
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            if qparams.get("ignore_wiring").map(|v| v == "1" || v == "true").unwrap_or(false) {
                dopts.ignore_wiring = true;
            }
            let new_only = qparams.get("new_only").map(|v| v == "1" || v == "true").unwrap_or(false);

            let mut rcfg = RouteConfig::default();
            if let Some(it) = qparams.get("iters").and_then(|s| s.parse().ok()) {
                rcfg.iters = it;
            }

            match parse_dsn(&dsn_text, &dopts) {
                Ok((inp, meta)) => {
                    let out = route_input(inp, &rcfg);
                    let ses = write_ses_string(&meta, &out, new_only, (0.0, 0.0));
                    let hdrs = [
                        ("X-EasyTrace-Routed-Conns", out.stats.conns_routed.to_string()),
                        ("X-EasyTrace-Total-Conns", out.stats.conns_total.to_string()),
                    ];
                    write_http_response(
                        &mut stream,
                        "200 OK",
                        "text/plain; charset=utf-8",
                        &hdrs,
                        ses.as_bytes(),
                    );
                }
                Err(e) => {
                    let err = serde_json::json!({"ok": false, "error": e});
                    write_http_response(
                        &mut stream,
                        "400 Bad Request",
                        "application/json",
                        &[],
                        &serde_json::to_vec(&err).unwrap(),
                    );
                }
            }
        }
        ("POST", "/route-geom") => {
            let body_str = String::from_utf8_lossy(&body);
            match serde_json::from_str::<Input>(&body_str) {
                Ok(inp) => {
                    let mut rcfg = RouteConfig::default();
                    if let Some(it) = qparams.get("iters").and_then(|s| s.parse().ok()) {
                        rcfg.iters = it;
                    }
                    let out = route_input(inp, &rcfg);
                    let mut resp = out.to_routes_json();
                    resp["ok"] = serde_json::json!(true);
                    write_http_response(
                        &mut stream,
                        "200 OK",
                        "application/json",
                        &[],
                        &serde_json::to_vec(&resp).unwrap(),
                    );
                }
                Err(e) => {
                    let err = serde_json::json!({"ok": false, "error": format!("Invalid geom JSON: {}", e)});
                    write_http_response(
                        &mut stream,
                        "400 Bad Request",
                        "application/json",
                        &[],
                        &serde_json::to_vec(&err).unwrap(),
                    );
                }
            }
        }
        _ => {
            let err = serde_json::json!({"ok": false, "error": format!("Unknown route: {} {}", method, route_path)});
            write_http_response(
                &mut stream,
                "404 Not Found",
                "application/json",
                &[],
                &serde_json::to_vec(&err).unwrap(),
            );
        }
    }
}

pub fn run_server(host: &str, port: u16) -> Result<(), String> {
    let addr = format!("{}:{}", host, port);
    let listener =
        TcpListener::bind(&addr).map_err(|e| format!("Failed to bind {}: {}", addr, e))?;
    eprintln!("================================================================");
    eprintln!("  EasyTrace-rs v{} Local Autorouter Server", env!("CARGO_PKG_VERSION"));
    eprintln!("  Listening on: http://{}", addr);
    eprintln!("  Endpoints:");
    eprintln!("    GET  /health        - Health check");
    eprintln!("    POST /route-jlceda  - JLCEDA Pro One-Click Extension endpoint");
    eprintln!("    POST /route-dsn     - Specctra .dsn -> .ses HTTP endpoint");
    eprintln!("    POST /route-geom    - Native EasyTrace geometry JSON endpoint");
    eprintln!("================================================================");

    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                std::thread::spawn(move || {
                    handle_client(stream);
                });
            }
            Err(e) => {
                eprintln!("[serve] accept error: {}", e);
            }
        }
    }
    Ok(())
}
