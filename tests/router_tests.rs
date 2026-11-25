use easytrace_rs::dsn::{parse_dsn, write_ses_string, DsnOptions};
use easytrace_rs::engine::route_input;
use easytrace_rs::jlceda::{parse_autoroute_json, write_jlceda_ses, JlcedaOptions};
use easytrace_rs::model::{Board, Input, Item, NetIn, RouteConfig, Rules};

#[test]
fn test_crossing_nets_two_layer_via_negotiation() {
    // Two crossing nets on a 10x10mm 2-layer board: PathFinder must negotiate a clean via crossing
    // with zero DRC violations and width >= 0.127mm.
    let outer = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
    let mk_pad = |net: u32, cx: f64, cy: f64, name: &str| Item {
        net,
        layers: vec![0],
        kind: "pad".to_string(),
        poly: vec![
            [cx - 0.3, cy - 0.3],
            [cx + 0.3, cy - 0.3],
            [cx + 0.3, cy + 0.3],
            [cx - 0.3, cy + 0.3],
        ],
        center: [cx, cy],
        name: name.to_string(),
        ..Default::default()
    };

    let items = vec![
        mk_pad(1, 2.0, 5.0, "U1_1"),
        mk_pad(1, 8.0, 5.0, "U1_2"),
        mk_pad(2, 5.0, 2.0, "U2_1"),
        mk_pad(2, 5.0, 8.0, "U2_2"),
    ];
    let nets = vec![
        NetIn {
            id: 1,
            name: "NET_H".to_string(),
            width: 0.127,
            cls: 0,
            islands: vec![vec![0], vec![1]],
        },
        NetIn {
            id: 2,
            name: "NET_V".to_string(),
            width: 0.200,
            cls: 1,
            islands: vec![vec![2], vec![3]],
        },
    ];
    let inp = Input {
        h: 0.05,
        layers: vec![1, 2],
        rules: Rules {
            clr_tt: 0.102,
            clr_other: 0.102,
            edge: 0.30,
            slot_track: 0.31,
            slot_via: 0.19,
            via_d: 0.50,
            via_hole_d: 0.30,
            hole_clr: 0.305,
            via_cost: 24.0,
            bend_cost: 1.8,
            widths: vec![0.127, 0.200],
        },
        items,
        nets,
        board: Board {
            outer,
            holes: Vec::new(),
        },
    };

    let mut cfg = RouteConfig::default();
    cfg.quiet = true;
    let out = route_input(inp, &cfg);
    assert_eq!(out.stats.conns_routed, 2);
    assert_eq!(out.stats.nets_routed, 2);
    assert!(out.fail.is_empty());
    for seg in &out.segs {
        assert!(
            seg.width >= 0.127 - 1e-6,
            "Expected width >= 0.127mm by default, got {}",
            seg.width
        );
    }
}

#[test]
fn test_specctra_dsn_roundtrip_and_string_quote_directive() {
    let dsn = r#"
(pcb "demo_board"
  (parser (string_quote ") (space_in_quoted_tokens on) (host_cad "EasyEDA Pro") (host_version "4.1.71"))
  (resolution um 10)
  (unit um)
  (structure
    (layer TopLayer (type signal))
    (layer BottomLayer (type signal))
    (boundary (path pcb 0 0 0 8000 0 8000 8000 0 8000 0 0))
    (via "VIA500")
    (rule (width 127) (clearance 102))
  )
  (placement
    (component R0402
      (place R1 2000 4000 front 0)
      (place R2 6000 4000 back 90)
    )
  )
  (library
    (image R0402
      (pin PS_SMD 1 -500 0)
      (pin PS_SMD 2 500 0)
    )
    (padstack PS_SMD
      (shape (rect TopLayer -250 -250 250 250))
    )
    (padstack "VIA500"
      (shape (circle TopLayer 500))
      (shape (circle BottomLayer 500))
    )
  )
  (network
    (net "SIG_A" (pins R1-2 R2-1))
    (net "VBAT" (pins R1-1 R2-2))
  )
  (wiring)
)
"#;
    let opts = DsnOptions::default();
    let (inp, meta) = parse_dsn(dsn, &opts).expect("DSN should parse cleanly");
    assert_eq!(meta.layer_names, vec!["TopLayer", "BottomLayer"]);
    assert_eq!(inp.nets.len(), 2);

    let mut cfg = RouteConfig::default();
    cfg.quiet = true;
    let out = route_input(inp, &cfg);
    assert_eq!(out.stats.conns_routed, 2);

    let ses = write_ses_string(&meta, &out, false, (0.0, 0.0));
    assert!(ses.contains("(session demo_board"));
    assert!(ses.contains("(network_out"));
    assert!(ses.contains("(net SIG_A"));
    assert!(ses.contains("(net VBAT"));
}

#[test]
fn test_jlceda_autoroute_json_arcs_cutouts_and_bottom_mirror() {
    // Includes 3-element `[x, y, sweep_deg]` circular pad paths, a central prohibitedRegion cutout
    // forcing a detour, and a bottom-layer (`layer: 2`) rotated component (`MIRROR_Y` before CCW rot).
    let sample = r#"{
      "boardOutline": {
        "bbox": [-6.0, 6.0, -6.0, 6.0],
        "path": [[-6.0, -6.0], [6.0, -6.0], [6.0, 6.0], [-6.0, 6.0], [-6.0, -6.0]]
      },
      "layers": { "route": [1, 2], "notRoute": [] },
      "rules": {
        "safeClearances": {
          "copperThickness1oz": [{
            "trackToTrack": 0.1,
            "trackToVia": 0.1,
            "trackToPad": 0.1,
            "trackToFillRegion": 0.1,
            "trackToProhibitedRegion": 0.3,
            "trackToBoardOutline": 0.3,
            "viaToVia": 0.1,
            "viaToPad": 0.1,
            "viaToProhibitedRegion": 0.16,
            "viaToBoardOutline": 0.3
          }]
        },
        "trackWidths": {
          "copperThickness1oz": [{ "trackWidth": [0.127, 0.127, 2.54] }]
        },
        "viaSizes": { "viaSize": [0.5, 0.3] }
      },
      "nets": [
        { "net": "SDA", "routing": true, "safeClearance": "copperThickness1oz", "trackWidth": "copperThickness1oz", "viaSize": "viaSize" },
        { "net": "GND", "routing": true, "safeClearance": "copperThickness1oz", "trackWidth": "copperThickness1oz", "viaSize": "viaSize" }
      ],
      "footprints": {
        "TP05": {
          "pads": {
            "e1": {
              "number": "1",
              "layers": [1],
              "location": [0.0, 1.0],
              "path": [[0.25, 1.0], [-0.25, 1.0, -180], [0.25, 1.0, -180]],
              "diameter": 0.5
            }
          }
        }
      },
      "components": {
        "TP1": { "name": "TP", "footprint": "TP05", "layer": 1, "location": [-4.0, -1.0], "rotation": 0, "nets": { "e1": "SDA" } },
        "TP2": { "name": "TP", "footprint": "TP05", "layer": 2, "location": [4.0, 1.0], "rotation": 0, "nets": { "e1": "SDA" } }
      },
      "prohibitedRegions": [
        { "path": [[-1.0, -2.0], [1.0, -2.0], [1.0, 2.0], [-1.0, 2.0], [-1.0, -2.0]], "layers": [1, 2] }
      ],
      "tracks": [],
      "vias": [],
      "fillRegions": []
    }"#;

    let jopts = JlcedaOptions::default();
    let (inp, meta) = parse_autoroute_json(sample, &jopts).expect("AutoRouteJson should parse");
    assert_eq!(inp.nets.len(), 1);
    assert_eq!(inp.nets[0].name, "SDA");

    let mut cfg = RouteConfig::default();
    cfg.quiet = true;
    let out = route_input(inp, &cfg);
    assert_eq!(out.stats.conns_routed, 1);
    assert!(!out.vias.is_empty(), "Top-to-bottom connection must place a via");

    // Verify no segment enters the central prohibited region `[-1, 1] x [-2, 2]`
    for seg in &out.segs {
        let mx = (seg.x1 + seg.x2) / 2.0;
        let my = (seg.y1 + seg.y2) / 2.0;
        assert!(
            !(mx > -1.0 && mx < 1.0 && my > -2.0 && my < 2.0),
            "Segment midpoint ({}, {}) entered prohibited cutout!",
            mx,
            my
        );
    }

    let ses = write_jlceda_ses(&meta, &out, true);
    assert!(ses.contains("(net SDA"));
}
