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

const MAX_HTTP_LINE_BYTES: usize = 8 * 1024;

fn read_line_limited<R: BufRead>(reader: &mut R, buf: &mut Vec<u8>) -> Result<usize, String> {
    buf.clear();
    let n = reader
        .by_ref()
        .take((MAX_HTTP_LINE_BYTES + 1) as u64)
        .read_until(b'\n', buf)
        .map_err(|e| format!("Failed reading HTTP line: {}", e))?;
    if n > MAX_HTTP_LINE_BYTES || (n == MAX_HTTP_LINE_BYTES && !buf.ends_with(b"\n")) {
        return Err("HTTP line exceeds maximum length".to_string());
    }
    Ok(n)
}

fn strip_eol(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

fn read_chunked_body<R: BufRead>(reader: &mut R) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    let mut line = Vec::new();
    loop {
        let n = read_line_limited(reader, &mut line)?;
        if n == 0 {
            return Err("Unexpected EOF while reading chunk size".to_string());
        }
        let size_part = strip_eol(&line)
            .split(|&b| b == b';')
            .next()
            .unwrap_or(b"");
        let start = size_part
            .iter()
            .position(|&b| b != b' ' && b != b'\t')
            .unwrap_or(size_part.len());
        let end = size_part
            .iter()
            .rposition(|&b| b != b' ' && b != b'\t')
            .map(|i| i + 1)
            .unwrap_or(start);
        let hex_bytes = &size_part[start..end];
        if hex_bytes.is_empty() || !hex_bytes.iter().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!(
                "Invalid chunk size: {:?}",
                String::from_utf8_lossy(hex_bytes)
            ));
        }
        let hex_str = std::str::from_utf8(hex_bytes).unwrap();
        let chunk_len = usize::from_str_radix(hex_str, 16)
            .map_err(|_| format!("Chunk size overflow: {:?}", hex_str))?;

        if chunk_len == 0 {
            loop {
                let tn = read_line_limited(reader, &mut line)?;
                if tn == 0 {
                    return Err("Unexpected EOF while reading chunked trailers".to_string());
                }
                if strip_eol(&line).is_empty() {
                    break;
                }
            }
            break;
        }

        let read = reader
            .by_ref()
            .take(chunk_len as u64)
            .read_to_end(&mut body)
            .map_err(|e| format!("Failed reading chunk payload: {}", e))?;
        if read != chunk_len {
            return Err("Unexpected EOF while reading chunk payload".to_string());
        }

        let dn = read_line_limited(reader, &mut line)?;
        if dn == 0 || (line != b"\r\n" && line != b"\n") {
            return Err("Malformed chunk delimiter".to_string());
        }
    }
    Ok(body)
}

fn handle_client(mut stream: TcpStream) {
    let Ok(peer) = stream.peer_addr() else { return };
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });

    let mut line_buf = Vec::new();
    match read_line_limited(&mut reader, &mut line_buf) {
        Ok(0) | Err(_) => return,
        Ok(_) => {}
    }
    let req_line = String::from_utf8_lossy(&line_buf).into_owned();
    if req_line.trim().is_empty() {
        return;
    }
    let parts: Vec<&str> = req_line.split_whitespace().collect();
    if parts.len() < 2 {
        return;
    }
    let method = parts[0].to_ascii_uppercase();
    let raw_target = parts[1];
    let (route_path, qparams) = parse_query(raw_target);

    let mut content_len: Option<usize> = None;
    let mut invalid_content_len = false;
    let mut transfer_codings: Vec<String> = Vec::new();
    let mut expect_continue = false;
    loop {
        match read_line_limited(&mut reader, &mut line_buf) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => {
                let err = serde_json::json!({"ok": false, "error": e});
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
        let line = String::from_utf8_lossy(&line_buf);
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        if let Some((hk, hv)) = trimmed.split_once(':') {
            let hname = hk.trim();
            let hval = hv.trim();
            if hname.eq_ignore_ascii_case("content-length") {
                match hval.parse::<usize>() {
                    Ok(n) => {
                        if let Some(prev) = content_len {
                            if prev != n {
                                invalid_content_len = true;
                            }
                        } else {
                            content_len = Some(n);
                        }
                    }
                    Err(_) => invalid_content_len = true,
                }
            } else if hname.eq_ignore_ascii_case("transfer-encoding") {
                for coding in hval
                    .split(',')
                    .map(|s| s.trim().to_ascii_lowercase())
                    .filter(|s| !s.is_empty())
                {
                    transfer_codings.push(coding);
                }
            } else if hname.eq_ignore_ascii_case("expect")
                && hval.eq_ignore_ascii_case("100-continue")
            {
                expect_continue = true;
            }
        }
    }

    if method == "OPTIONS" {
        write_http_response(&mut stream, "204 No Content", "text/plain", &[], b"");
        return;
    }

    let is_chunked = if !transfer_codings.is_empty() {
        if transfer_codings.len() == 1 && transfer_codings[0] == "chunked" {
            true
        } else {
            write_http_response(
                &mut stream,
                "400 Bad Request",
                "application/json",
                &[],
                br#"{"ok":false,"error":"Unsupported Transfer-Encoding"}"#,
            );
            return;
        }
    } else {
        if invalid_content_len {
            write_http_response(
                &mut stream,
                "400 Bad Request",
                "application/json",
                &[],
                br#"{"ok":false,"error":"Invalid Content-Length header"}"#,
            );
            return;
        }
        false
    };

    if expect_continue
        && method == "POST"
        && matches!(route_path, "/route-jlceda" | "/route-dsn" | "/route-geom")
    {
        let _ = stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n");
        let _ = stream.flush();
    }

    let body = if is_chunked {
        match read_chunked_body(&mut reader) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("[serve] chunked body error from {}: {}", peer, e);
                let err = serde_json::json!({"ok": false, "error": e});
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
    } else if let Some(cl) = content_len {
        let mut buf = Vec::new();
        if cl > 0 {
            let ok = reader
                .by_ref()
                .take(cl as u64)
                .read_to_end(&mut buf)
                .map(|n| n == cl)
                .unwrap_or(false);
            if !ok {
                write_http_response(
                    &mut stream,
                    "400 Bad Request",
                    "application/json",
                    &[],
                    br#"{"ok":false,"error":"Failed to read full request body"}"#,
                );
                return;
            }
        }
        buf
    } else {
        Vec::new()
    };

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn send_raw_request(raw: &[u8]) -> (String, String) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                handle_client(stream);
            }
        });

        let mut client = TcpStream::connect(addr).expect("connect test server");
        client.write_all(raw).expect("write request");
        let _ = client.shutdown(std::net::Shutdown::Write);
        let mut resp = Vec::new();
        client.read_to_end(&mut resp).expect("read response");
        server.join().expect("server thread panicked");

        let resp_str = String::from_utf8_lossy(&resp).into_owned();
        let (head, body) = resp_str.split_once("\r\n\r\n").unwrap_or((&resp_str, ""));
        (head.to_string(), body.to_string())
    }

    #[test]
    fn test_read_chunked_body_valid() {
        let raw = b"4;ext=foo\r\nWiki\r\n5\r\npedia\r\nE\r\n in\r\n\r\nchunks.\r\n0\r\nX-Trailer: 1\r\n\r\n";
        let mut cur = Cursor::new(&raw[..]);
        let body = read_chunked_body(&mut cur).expect("valid chunked body");
        assert_eq!(body, b"Wikipedia in\r\n\r\nchunks.");

        let mut empty = Cursor::new(&b"0\r\n\r\n"[..]);
        assert_eq!(read_chunked_body(&mut empty).unwrap(), b"");
    }

    #[test]
    fn test_read_chunked_body_rejects_malformed() {
        for bad in [
            &b"+4\r\nabcd\r\n0\r\n\r\n"[..],
            &b"zz\r\nabcd\r\n0\r\n\r\n"[..],
            &b"\r\n4\r\nabcd\r\n0\r\n\r\n"[..],
            &b"3\r\nabcXYZ\r\n0\r\n\r\n"[..],
            &b"5\r\nabc"[..],
            &b"0\r\n"[..],
        ] {
            let mut cur = Cursor::new(bad);
            assert!(
                read_chunked_body(&mut cur).is_err(),
                "expected error for malformed chunked input: {:?}",
                String::from_utf8_lossy(bad)
            );
        }
    }

    #[test]
    fn test_handle_client_chunked_post_route_jlceda() {
        let payload = serde_json::json!({
            "autoRoute": {
                "boardOutline": {
                    "bbox": [-5.0, 5.0, -5.0, 5.0],
                    "path": [[-5.0, -5.0], [5.0, -5.0], [5.0, 5.0], [-5.0, 5.0], [-5.0, -5.0]]
                },
                "layers": { "route": [1, 2], "notRoute": [] },
                "rules": {},
                "nets": [{ "net": "SIG1", "routing": true }],
                "footprints": {
                    "P1": { "pads": { "e1": { "number": "1", "layers": [1], "location": [0.0, 0.0], "diameter": 0.5 } } }
                },
                "components": {
                    "U1": { "name": "U1", "footprint": "P1", "layer": 1, "location": [-2.0, 0.0], "rotation": 0.0, "nets": { "e1": "SIG1" } },
                    "U2": { "name": "U2", "footprint": "P1", "layer": 1, "location": [2.0, 0.0], "rotation": 0.0, "nets": { "e1": "SIG1" } }
                },
                "prohibitedRegions": [],
                "tracks": [],
                "vias": [],
                "fillRegions": []
            },
            "options": { "iters": 5, "h": 0.05, "skip": ["GND"] }
        })
        .to_string();
        let bytes = payload.as_bytes();
        let (c1, rest) = bytes.split_at(bytes.len() / 3);
        let (c2, c3) = rest.split_at(rest.len() / 2);

        let mut req = Vec::new();
        req.extend_from_slice(
            b"POST /route-jlceda HTTP/1.1\r\n\
              Host: 127.0.0.1:3789\r\n\
              Content-Type: application/json\r\n\
              Transfer-Encoding: chunked\r\n\r\n",
        );
        for chunk in [c1, c2, c3] {
            req.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
            req.extend_from_slice(chunk);
            req.extend_from_slice(b"\r\n");
        }
        req.extend_from_slice(b"0\r\n\r\n");

        let (head, body) = send_raw_request(&req);
        assert!(head.starts_with("HTTP/1.1 200 OK"), "unexpected head: {}", head);
        let json: serde_json::Value = serde_json::from_str(&body).expect("valid JSON response");
        assert_eq!(json["ok"], serde_json::json!(true));
        assert_eq!(json["stats"]["conns_routed"], serde_json::json!(1));
    }

    #[test]
    fn test_handle_client_rejects_bad_headers() {
        let (head_cl, _) = send_raw_request(
            b"POST /route-jlceda HTTP/1.1\r\nHost: 127.0.0.1:3789\r\nContent-Length: abc\r\n\r\n",
        );
        assert!(head_cl.starts_with("HTTP/1.1 400 Bad Request"));

        let (head_te, _) = send_raw_request(
            b"POST /route-jlceda HTTP/1.1\r\nHost: 127.0.0.1:3789\r\nTransfer-Encoding: gzip\r\n\r\n",
        );
        assert!(head_te.starts_with("HTTP/1.1 400 Bad Request"));
    }
}
