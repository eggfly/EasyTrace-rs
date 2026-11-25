"use strict";
var edaEsbuildExportName = (function () {
	var exports = {};
	var MM_TO_MIL = 1.0 / 0.0254;
	var DEFAULT_SERVER = "http://127.0.0.1:3789";

	function toast(msg, kind) {
		try {
			var t =
				typeof ESYS_ToastMessageType !== "undefined"
					? ESYS_ToastMessageType[kind || "INFO"]
					: kind === "ERROR"
					? "error"
					: "info";
			eda.sys_ToastMessage.showMessage(msg, t);
		} catch (e) {
			console.log("[EasyTrace-rs]", msg);
		}
	}

	function activate() {}

	function openEasyTracePanel() {
		eda.sys_IFrame.openIFrame("/iframe/index.html", 430, 560, "easytrace-autoroute-dialog");
	}

	async function quickAutoRoute() {
		try {
			toast("EasyTrace-rs: Exporting PCB geometry (with cutouts & NPTH slots)...", "INFO");
			var jrFile = await eda.pcb_ManufactureData.getAutoRouteJsonFile();
			if (!jrFile) {
				toast("EasyTrace-rs: getAutoRouteJsonFile() returned empty — open a PCB document first.", "ERROR");
				return;
			}
			var rawJson = typeof jrFile === "string" ? jrFile : await jrFile.text();
			var resp = await fetch(DEFAULT_SERVER + "/route-jlceda", {
				method: "POST",
				headers: { "Content-Type": "application/json" },
				body: rawJson,
			});
			if (!resp.ok) {
				var errTxt = await resp.text();
				throw new Error("HTTP " + resp.status + ": " + errTxt);
			}
			var data = await resp.json();
			if (!data.ok) {
				throw new Error(data.error || "Routing failed");
			}
			var lines = data.lines || [];
			var vias = data.vias || [];
			var defaultHoleMm = data.viaHole || 0.3;
			var nLines = 0;
			var nVias = 0;

			for (var i = 0; i < lines.length; i++) {
				var seg = lines[i];
				var okL = await eda.pcb_PrimitiveLine.create(
					seg[0],
					seg[1],
					+(seg[2] * MM_TO_MIL).toFixed(3),
					+(seg[3] * MM_TO_MIL).toFixed(3),
					+(seg[4] * MM_TO_MIL).toFixed(3),
					+(seg[5] * MM_TO_MIL).toFixed(3),
					+(seg[6] * MM_TO_MIL).toFixed(3),
					false
				);
				if (okL) nLines++;
			}
			for (var j = 0; j < vias.length; j++) {
				var v = vias[j];
				var odMm = v[3] || 0.5;
				var holeMm = odMm <= 0.451 ? 0.2 : defaultHoleMm;
				var okV = await eda.pcb_PrimitiveVia.create(
					v[0],
					+(v[1] * MM_TO_MIL).toFixed(3),
					+(v[2] * MM_TO_MIL).toFixed(3),
					+(holeMm * MM_TO_MIL).toFixed(2),
					+(odMm * MM_TO_MIL).toFixed(3)
				);
				if (okV) nVias++;
			}

			var st = data.stats || {};
			toast(
				"EasyTrace-rs Done (" +
					(st.elapsed_s || 0) +
					"s): " +
					(st.conns_routed || 0) +
					"/" +
					(st.conns_total || 0) +
					" conns, +" +
					nLines +
					" segs, +" +
					nVias +
					" vias",
				"INFO"
			);
		} catch (err) {
			console.error("[EasyTrace-rs]", err);
			toast(
				"EasyTrace-rs Error: " +
					(err && err.message ? err.message : String(err)) +
					" (Is `easytrace serve` running?)",
				"ERROR"
			);
		}
	}

	exports.activate = activate;
	exports.openEasyTracePanel = openEasyTracePanel;
	exports.quickAutoRoute = quickAutoRoute;

	return exports;
})();
