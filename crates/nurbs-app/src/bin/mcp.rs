//! forme-nurbs-mcp: the MCP server Claude Desktop starts (stdio, JSON-RPC
//! 2.0, one message per line). Each tool call is passed to the running app
//! on 127.0.0.1:47830 (FORME_NURBS_PORT; `remote.rs`); when the app is not
//! running it is started from the folder of this program.
//!
//! A call that waits long (the part being built, the deviation measured)
//! sends progress notifications when the client asked for them, and after
//! FORME_NURBS_WAIT seconds (50) answers "still computing" while the app goes
//! on: clients give up on a tool call after about a minute.
//!
//! Claude Desktop's config (claude_desktop_config.json):
//! { "mcpServers": { "forme-nurbs": { "command": "C:\\...\\forme-nurbs-mcp.exe" } } }

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

const PORT: u16 = 47830;

const INSTRUCTIONS: &str = "Forme NURBS is a hard-surface modeller (OpenCascade B-rep): a part is a list of features in order (Add = union, Carve = cut), cubes, cylinders and sketches (a flat outline pulled to a height), then fillets and chamfers. \
Here it rebuilds an AI-made mesh (the reference, a scan) as clean shapes. THE RULE (Romain): interpret, do not match. Every shape is ideal and machinable: a straight cylinder has one radius, a block stays a block (at most slightly tapered), a fillet has one constant radius (the median of what is read, never a variable radius). The scan's waves and warps are noise: say when a read varies, never follow it. Prefer round values, symmetry (the part's flanks), series (equal vents, equal rivets), shared axes and planes. \
Work from the general to the detail: 1 primary (the big volumes: receiver, barrel block, grip; they give the part's axes and symmetry), 2 secondary (the big fillets and bevels along them, the big cuts, the double barrel), 3 tertiary (buttons, rivets, vents, small pockets laid on the primaries' faces). After each stage, the deviation says what is left: read again, finer, there. \
Do not model the cables and hoses (Romain places them by hand): leave those zones unexplained. \
The loop: the app measures and proposes (primary_candidates, measure), you choose, merge, idealise and add the shapes (add_features: one undo step per call), then check with deviation and capture. Secondary fillets: edge_candidates reads the scan across the part's sharp edges, add_fillets makes them by groups (one radius for edges that belong together, the biggest first; it says the size made), deviation's elements compare a block with its fillets (all by area). Units are millimetres, Y is up. A shape spec: {op: add|carve, kind: cube|cylinder|sketch, origin: the base's centre [x,y,z], x_axis: local X, up: the way it rises (local Y), cube: size [x,y,z]; cylinder: radius, height; sketch: outline [[x,z],...] in mm in its plane (x along x_axis, z along x_axis × up), corners (per point: a fillet radius > 0, a chamfer < 0, 0 sharp), height}. \
Look before deciding: capture (view front/back/left/right/top/3d, reference solid/ghost/hidden, deviation true), then pick on the picture to know what is where. Show the user what you did with show (it moves their view). Everything can be undone (undo).";

fn tools() -> Value {
    let v3 = json!({ "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3 });
    let spec = json!({
        "type": "object",
        "description": "a shape: {op: add|carve, kind: cube|cylinder|sketch, name?, origin: [x,y,z] the base's centre, x_axis: [x,y,z] or \"+X\", up: [x,y,z] or \"+Z\" (the way it rises), cube: size [x,y,z] (along x_axis, up, x_axis × up); cylinder: radius, height; sketch: outline [[x,z],...] mm in its plane, corners [per point: fillet radius > 0, chamfer < 0, 0 sharp], height}",
    });
    let view = json!({
        "view": { "type": "string", "enum": ["front", "back", "left", "right", "top", "bottom", "3d"], "description": "framed on the part and the scan; front looks along -Z (from +Z), right from +X" },
        "yaw": { "type": "number", "description": "degrees round Y (0 = from +Z, 90 = from +X)" },
        "pitch": { "type": "number", "description": "degrees up" },
        "ortho": { "type": "boolean" },
        "focus": v3, "distance": { "type": "number", "description": "mm from the focus" },
        "frame_ids": { "type": "array", "items": { "type": "integer" }, "description": "frame these features" },
        "frame_box": { "type": "array", "description": "[[lo], [hi]]: frame this box" },
        "reference": { "type": "string", "enum": ["solid", "ghost", "hidden"] },
        "deviation": { "type": "boolean", "description": "colour the part by its distance to the scan (green within the tolerance, yellow to red outside, cyan to blue inside, grey past 5 mm); the reference is then hidden unless named" },
    });
    let mut cap = view.clone();
    cap["width"] = json!({ "type": "integer" });
    cap["height"] = json!({ "type": "integer" });
    let mut sh = view.clone();
    sh["select"] = json!({ "type": "array", "items": { "type": "integer" } });
    json!([
        { "name": "status", "description": "What the app has open: the document, its features count, the reference (path, bounds, grain), whether the part is building, its errors, the undo list, the camera.", "inputSchema": { "type": "object", "properties": {} } },
        { "name": "open_reference", "description": "Loads an .obj as the reference (the scan to rebuild); starts the app if needed. It loads in the background (a few seconds for 2 M triangles).", "inputSchema": { "type": "object", "properties": { "path": { "type": "string", "description": "full path of the .obj" } }, "required": ["path"] } },
        { "name": "primary_candidates", "description": "Stage 1: the scan's big blocks read as slabs seen from its side (each a sketch spec with corners, its back and front levels, its area, how much of it is flat). Nothing is added unless apply is true (then they are added as the app's Primary shapes button does). Usually: take them, idealise (round values, flanks symmetric about the part's middle, square sides), then add_features.", "inputSchema": { "type": "object", "properties": { "apply": { "type": "boolean" } } } },
        { "name": "list_features", "description": "The document's features in order, as specs (and for steps: fillet / chamfer size and edge points, inset / extrude, moves), with bounds; the model's errors.", "inputSchema": { "type": "object", "properties": { "ids": { "type": "array", "items": { "type": "integer" } } } } },
        { "name": "add_features", "description": "Adds shapes (specs) at the end of the history, in one undo step (label). Returns their ids. The part is rebuilt in the background.", "inputSchema": { "type": "object", "properties": { "features": { "type": "array", "items": spec }, "label": { "type": "string" } }, "required": ["features"] } },
        { "name": "edit_feature", "description": "Changes one feature (one undo step): any spec field (op, origin, x_axis, up, size, radius, height, outline, corners, name, visible); for a fillet / chamfer its size, for an inset / extrude its inset and depth.", "inputSchema": { "type": "object", "properties": { "id": { "type": "integer" }, "op": { "type": "string" }, "origin": v3, "x_axis": {}, "up": {}, "size": {}, "radius": { "type": "number" }, "height": { "type": "number" }, "outline": { "type": "array" }, "corners": { "type": "array" }, "name": { "type": "string" }, "visible": { "type": "boolean" }, "inset": { "type": "number" }, "depth": { "type": "number" } }, "required": ["id"] } },
        { "name": "delete_features", "description": "Removes features (one undo step).", "inputSchema": { "type": "object", "properties": { "ids": { "type": "array", "items": { "type": "integer" } } }, "required": ["ids"] } },
        { "name": "undo", "description": "Undoes (or redoes) steps of the document's history.", "inputSchema": { "type": "object", "properties": { "steps": { "type": "integer" }, "redo": { "type": "boolean" } } } },
        { "name": "measure", "description": "Reads the scan at a place, as the app's tools do: kind round (a and b: rim to rim across a hole or a boss; stepped parts give tiers), slot (end to end along a slot or a raised pill), box (corner to corner across a block or a pocket), plane (a only: the flat face there, else the tangent plane). Points within 10 mm of the scan (they are moved onto it). Returns the read values and specs ready for add_features.", "inputSchema": { "type": "object", "properties": { "kind": { "type": "string", "enum": ["round", "slot", "box", "plane"] }, "a": v3, "b": v3, "reach": { "type": "number" } }, "required": ["kind", "a"] } },
        { "name": "edge_candidates", "description": "Stage 2 (secondary): every sharp edge of the part (at least min_length mm, 3) read on the scan as Fit to scan does: the fillet radius the scan shows across it (the median of its slices, their spread lo..hi, varies), convex or concave, its point (at), ends, length and the features of its two faces; the edges the scan keeps sharp; how many could not be read. features: only the edges touching these. Takes a few seconds.", "inputSchema": { "type": "object", "properties": { "features": { "type": "array", "items": { "type": "integer" } }, "min_length": { "type": "number" }, "limit": { "type": "integer" } } } },
        { "name": "add_fillets", "description": "Fillet (or chamfer) steps on the part's edges, named by points on them (an edge candidate's at, within 1.5 mm): one step per group (one radius, idealised: round values, one radius for edges that belong together), all in one undo step. Waits for the build and says the size made of each (made, stopped: a fillet too big for its edges stops at the largest that works, often at a short side or a small chamfer of the outline next to it).", "inputSchema": { "type": "object", "properties": { "groups": { "type": "array", "items": { "type": "object", "properties": { "at": { "type": "array", "items": v3 }, "radius": { "type": "number" }, "chamfer": { "type": "boolean" }, "name": { "type": "string" } }, "required": ["at", "radius"] } }, "label": { "type": "string" } }, "required": ["groups"] } },
        { "name": "deviation", "description": "Turns the deviation on and measures the part against the scan (waits for the part): all by area: overall (the share of the area measured, i.e. within 5 mm of the scan: a sharp corner far out of it is not; of that the shares within the tolerance, outside and inside, the median and 95th percentile), per feature, per element with operations (a block and its fillets together: compare it with the block before them; a fillet's own faces stay outside the scan by as much as the faces it joins), in a box when one is given (box: [[x,y,z],[x,y,z]], as region), and the zones still far from it (points past zone_min mm (0.5) on one face of the part, joined over zone_cell mm cells (3), largest first, with the face and its feature: outside = the part sticks out of the scan there (a face too far out, or a band along an edge the scan rounds off: a fillet or bevel to read), inside = the part is short of the scan (a face too far in, or a detail missing: a button, a boss, a hose). range: 0.1 to 2 mm (tolerance a fifth of it, at most 0.1).", "inputSchema": { "type": "object", "properties": { "range": { "type": "number" }, "zone_min": { "type": "number" }, "zone_cell": { "type": "number" }, "limit": { "type": "integer" }, "box": { "type": "array", "items": { "type": "array", "items": { "type": "number" } } } } } },
        { "name": "capture", "description": "A picture of the 3D view alone (no interface), waits for the part (and the deviation when asked). Without view options: the user's camera.", "inputSchema": { "type": "object", "properties": cap } },
        { "name": "pick", "description": "What lies under a spot of the last capture (x, y as fractions of the picture from its top left): the part's face, its feature and point; the scan's point.", "inputSchema": { "type": "object", "properties": { "x": { "type": "number" }, "y": { "type": "number" } }, "required": ["x", "y"] } },
        { "name": "show", "description": "Changes what the user sees in the app (camera, reference solid / ghost / hidden, deviation, selection), no picture.", "inputSchema": { "type": "object", "properties": sh } }
    ])
}

fn port() -> u16 {
    std::env::var("FORME_NURBS_PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(PORT)
}

/// Starts the app next to this program (once).
fn launch(mesh: Option<&str>) -> Result<(), String> {
    // FORME_NURBS_APP: the app's path (this server copied out of the build
    // folder, so that Claude Desktop holding it open does not stop a build),
    // else the app next to it
    let exe = match std::env::var_os("FORME_NURBS_APP") {
        Some(p) => std::path::PathBuf::from(p),
        None => {
            let me = std::env::current_exe().map_err(|e| e.to_string())?;
            me.parent().ok_or("no folder")?.join(if cfg!(windows) { "forme-nurbs.exe" } else { "forme-nurbs" })
        }
    };
    if !exe.exists() {
        return Err(format!("the app is not running and {} was not found", exe.display()));
    }
    let mut cmd = std::process::Command::new(exe);
    if let Some(m) = mesh {
        cmd.arg(m);
    }
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    cmd.spawn().map_err(|e| e.to_string())?;
    Ok(())
}

fn connect(start_with: Option<&str>) -> Result<(TcpStream, bool), String> {
    if let Ok(s) = TcpStream::connect(("127.0.0.1", port())) {
        return Ok((s, false));
    }
    launch(start_with)?;
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(30) {
        std::thread::sleep(Duration::from_millis(300));
        if let Ok(s) = TcpStream::connect(("127.0.0.1", port())) {
            return Ok((s, true));
        }
    }
    Err("the app did not answer (started, but no link after 30 s)".into())
}

/// One request to the app, its answer.
fn call_app(method: &str, params: Value) -> Result<Value, String> {
    let start_with = if method == "open_reference" { params["path"].as_str().map(|s| s.to_string()) } else { None };
    let (mut s, started) = connect(start_with.as_deref())?;
    // the app started with the mesh on its command line has it already
    if started && method == "open_reference" {
        return Ok(json!({ "loading": start_with, "note": "the app was started with it" }));
    }
    s.set_read_timeout(Some(Duration::from_secs(900))).ok();
    writeln!(s, "{}", json!({ "method": method, "params": params })).map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&line).map_err(|e| format!("bad answer from the app: {e}"))?;
    if let Some(e) = v.get("error") {
        return Err(e.as_str().unwrap_or("error").to_string());
    }
    Ok(v.get("ok").cloned().unwrap_or(Value::Null))
}

fn text(v: &Value) -> Value {
    json!({ "type": "text", "text": serde_json::to_string(v).unwrap_or_default() })
}

/// `call_app` on a thread: progress notifications every 5 s (when the
/// client gave a token), "still computing" after the wait limit.
fn call_app_waiting(method: &str, params: Value, progress: Option<&Value>) -> Result<Value, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let (m, p) = (method.to_string(), params);
    std::thread::spawn(move || {
        let _ = tx.send(call_app(&m, p));
    });
    let limit = std::env::var("FORME_NURBS_WAIT").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(50);
    let t0 = Instant::now();
    let mut n = 0u64;
    loop {
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(r) => return r,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Err("the link to the app broke".into()),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if t0.elapsed() >= Duration::from_secs(limit) {
                    return Ok(json!({
                        "still_computing": true,
                        "note": format!("the app is still working after {limit} s and goes on: call status until building is false, then call {method} again"),
                    }));
                }
                if let Some(tok) = progress {
                    n += 1;
                    let mut out = std::io::stdout();
                    let _ = writeln!(out, "{}", json!({ "jsonrpc": "2.0", "method": "notifications/progress", "params": { "progressToken": tok, "progress": n, "message": "working" } }));
                    let _ = out.flush();
                }
            }
        }
    }
}

const TOOLS: &[&str] = &["status", "open_reference", "primary_candidates", "list_features", "add_features", "edit_feature", "delete_features", "undo", "measure", "edge_candidates", "add_fillets", "deviation", "capture", "pick", "show"];

fn call_tool(name: &str, args: Value, progress: Option<&Value>) -> Result<Vec<Value>, String> {
    if !TOOLS.contains(&name) {
        return Err(format!("unknown tool {name}"));
    }
    let asked = (name == "add_fillets").then(|| args.get("groups").cloned()).flatten();
    let mut v = call_app_waiting(name, args, progress)?;
    if let (Some(groups), Some(ids)) = (asked, v.get("ids").and_then(|v| v.as_array()).cloned()) {
        // the sizes made, once built: a fillet too big for its edges stops
        // at the largest that works
        if let Ok(list) = call_app_waiting("list_features", json!({}), progress) {
            let feats = list.get("features").and_then(|f| f.as_array()).cloned().unwrap_or_default();
            let mut made = vec![];
            for (k, id) in ids.iter().enumerate() {
                let Some(f) = feats.iter().find(|f| f.get("id") == Some(id)) else { continue };
                let want = groups.get(k).and_then(|g| g.get("radius").or(g.get("size"))).and_then(|r| r.as_f64()).unwrap_or(0.0);
                let size = f.get("size").and_then(|s| s.as_f64()).unwrap_or(0.0);
                made.push(json!({ "id": id, "name": f.get("name"), "asked": want, "made": size, "stopped": size < want - 1e-3 }));
            }
            if made.iter().any(|m| m["stopped"] == json!(true)) {
                v["stopped_note"] = json!("stopped: the largest that works on those edges. Usual causes: a short side or a small chamfer of the outline next to the edge (the fillet cannot turn round it: take the edges of the whole chain in one group, or edit the outline), the fillets of one group meeting, a face too thin for that radius.");
            }
            v["made"] = json!(made);
            if let Some(e) = list.get("errors").filter(|e| e.as_array().is_some_and(|a| !a.is_empty())) {
                v["model_errors"] = e.clone();
            }
        }
    }
    if name == "capture" && v.get("png_base64").is_some() {
        let data = v["png_base64"].as_str().unwrap_or("").to_string();
        if let Some(o) = v.as_object_mut() {
            o.remove("png_base64");
        }
        return Ok(vec![json!({ "type": "image", "data": data, "mimeType": "image/png" }), text(&v)]);
    }
    Ok(vec![text(&v)])
}

fn handle(msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg["method"].as_str().unwrap_or("");
    let result: Result<Value, (i64, String)> = match method {
        "initialize" => {
            let asked = msg["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18").to_string();
            Ok(json!({
                "protocolVersion": asked,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "forme-nurbs", "version": env!("CARGO_PKG_VERSION") },
                "instructions": INSTRUCTIONS,
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => {
            let name = msg["params"]["name"].as_str().unwrap_or("");
            let args = msg["params"].get("arguments").cloned().unwrap_or(json!({}));
            let progress = msg["params"]["_meta"].get("progressToken");
            match call_tool(name, args, progress) {
                Ok(content) => Ok(json!({ "content": content, "isError": false })),
                Err(e) => Ok(json!({ "content": [{ "type": "text", "text": e }], "isError": true })),
            }
        }
        "resources/list" => Ok(json!({ "resources": [] })),
        "prompts/list" => Ok(json!({ "prompts": [] })),
        m if m.starts_with("notifications/") => return None,
        m => Err((-32601, format!("method not found: {m}"))),
    };
    let id = id?;
    Some(match result {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
        Err((code, message)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
    })
}

fn main() {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let _ = writeln!(out, "{}", json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": e.to_string() } }));
                let _ = out.flush();
                continue;
            }
        };
        if let Some(reply) = handle(&msg) {
            let _ = writeln!(out, "{reply}");
            let _ = out.flush();
        }
    }
}
