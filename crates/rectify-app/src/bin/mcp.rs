//! forme-rectify-mcp: the MCP server Claude Desktop starts (stdio, JSON-RPC
//! 2.0, one message per line). Each tool call is passed to the running app
//! on 127.0.0.1:47820 (FORME_RECTIFY_PORT); when the app is not running it
//! is started from the folder of this program.
//!
//! A call that takes long (a stage being computed) sends progress
//! notifications when the client asked for them, and after
//! FORME_RECTIFY_WAIT seconds (50) answers "still computing" while the app
//! goes on: clients give up on a tool call after about a minute.
//!
//! Claude Desktop's config (claude_desktop_config.json):
//! { "mcpServers": { "forme-rectify": { "command": "C:\\...\\forme-rectify-mcp.exe" } } }

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

const PORT: u16 = 47820;

const INSTRUCTIONS: &str = "Forme Rectify cleans a noisy mesh made by an AI (or a scan): its triangles stay, its vertices move onto the shapes the object is made of. \
The app shows 4 stages: 0 the scan, 1 the algorithm alone (regions = planes / cylinders / spheres, edges and fillets rebuilt, round pieces), 2 cleaned (nowhere rougher than the AI mesh), 3 the algorithm driven by edits: yours (the AI pass) and the user's brush strokes. \
Your job in the AI pass: look at the mesh (capture), read the regions (list_regions, region_info), and decide what the algorithm got wrong: regions that are one surface split in pieces (merge), wrong shape kinds (retype), organic parts that should stay as they are (free, keep), parts that should share a radius / axis / plane (same_radius, coaxial, coplanar), round turned pieces it missed (revolve) or wrongly made round (no_revolve). Then run stage 3 and compare it with stage 2 (capture both, same view, style surface or diff). \
Work in small batches and check each with a capture. Units are millimetres. The user cares about straight lines and clean fillets, not micro noise. \
What usually goes wrong: small repeated details (ribbed hoses, fins, grooves, knurls) are cut into many short cylinders and small planes, and the snap flattens their detail or leaves lumps where they meet: free all of their regions (list_regions near the part, keep those inside its box, leave the big faces of the frame around it). Stage 2 never gets rougher than the AI mesh but can lose such details: compare it with stage 0. \
To find what is where: capture, then pick on the picture (x, y as fractions); capture with edges false for a clean look, style regions to see the regions.";

fn tools() -> Value {
    let stage = json!({ "type": ["integer", "string"], "description": "0 scan, 1 algorithm, 2 cleaned, 3 ai (default 3)" });
    let v3 = json!({ "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3 });
    let regions = json!({ "type": "array", "items": { "type": ["integer", "object"] }, "description": "region ids (from list_regions, stage 3), or {at: [x,y,z]} points on them" });
    json!([
        { "name": "status", "description": "What the app has open, which stages are computed, how many edits, the camera.", "inputSchema": { "type": "object", "properties": {} } },
        { "name": "open_mesh", "description": "Opens an .obj in the app (starts the app if needed). Edits saved next to it (<name>.rectify.json) come back.", "inputSchema": { "type": "object", "properties": { "path": { "type": "string", "description": "full path of the .obj" } }, "required": ["path"] } },
        { "name": "run_stage", "description": "Computes a stage (only what is out of date) and shows it. Returns its numbers: regions, edges, round pieces, how many vertices are rougher than the AI mesh, and for stage 3 what each edit did. Optional settings change steps first, e.g. \"fade=2,tidy_margin=10\".", "inputSchema": { "type": "object", "properties": { "stage": stage, "settings": { "type": "string" } } } },
        { "name": "capture", "description": "A picture of the view (no interface). Choose the stage and style (surface: clay; regions: one colour per region, planes cool, cylinders warm, free faces dark; diff: grey untouched, orange moved up to 0.4 mm, red rougher than the AI mesh). Aim with view (3d, front, back, left, right, top, bottom, all), yaw / pitch in degrees, focus [x,y,z] and distance (mm), or region (an id: frames it). zebra: stripes that show how true the surfaces are.", "inputSchema": { "type": "object", "properties": { "stage": stage, "style": { "type": "string", "enum": ["surface", "regions", "diff"] }, "view": { "type": "string" }, "yaw": { "type": "number" }, "pitch": { "type": "number" }, "focus": v3, "distance": { "type": "number" }, "region": { "type": "integer" }, "zebra": { "type": "boolean" }, "edges": { "type": "boolean" }, "width": { "type": "integer" }, "height": { "type": "integer" } } } },
        { "name": "show", "description": "Changes what the user sees in the app (same options as capture, without a picture).", "inputSchema": { "type": "object", "properties": { "stage": stage, "style": { "type": "string" }, "view": { "type": "string" }, "yaw": { "type": "number" }, "pitch": { "type": "number" }, "focus": v3, "distance": { "type": "number" }, "region": { "type": "integer" }, "zebra": { "type": "boolean" } } } },
        { "name": "pick", "description": "What is under a spot of the last capture: x, y as fractions of the picture from its top left (0-1), with the same width / height as that capture. Gives the 3D point and the stage-3 region there (id, kind, parameters).", "inputSchema": { "type": "object", "properties": { "x": { "type": "number" }, "y": { "type": "number" }, "width": { "type": "integer" }, "height": { "type": "integer" } }, "required": ["x", "y"] } },
        { "name": "list_regions", "description": "The regions (largest first): id, kind and parameters (plane normal / offset, cylinder axis / radius, sphere centre / radius), area, fit error (rms), a point on it (at), its box. Stage 3 (default) = the edited regions your edits refer to. Filter by kind, min_area (mm²), near [x,y,z] within radius (mm).", "inputSchema": { "type": "object", "properties": { "stage": stage, "kind": { "type": "string", "enum": ["plane", "cylinder", "sphere"] }, "min_area": { "type": "number" }, "near": v3, "radius": { "type": "number" }, "limit": { "type": "integer" } } } },
        { "name": "region_info", "description": "One region and the regions it touches (by shared border length): to find pieces of one surface split apart.", "inputSchema": { "type": "object", "properties": { "id": { "type": "integer" }, "stage": stage }, "required": ["id"] } },
        { "name": "edit_regions", "description": "Adds edits to stage 3 (applied in order, saved next to the mesh, undoable by remove_edit). Each edit is an object with op and a note saying why. Ops: {op: merge, regions: [ids], kind?: plane|cylinder|sphere} one surface; {op: retype, region: id, kind: plane|cylinder|sphere|free}; {op: free, regions: [ids]} left as the AI mesh by the snap; {op: same_radius, regions: [ids], radius?}; {op: coaxial, regions: [cylinder ids]}; {op: coplanar, regions: [plane ids]}; {op: revolve, at: [x,y,z], axis: [x,y,z], radius, depth?} a turned piece made round; {op: no_revolve, at: [x,y,z]}; {op: keep, at: [x,y,z], radius} a zone left exactly as the AI mesh. Then run_stage 3.", "inputSchema": { "type": "object", "properties": { "edits": { "type": "array", "items": { "type": "object", "properties": { "op": { "type": "string" }, "note": { "type": "string" }, "regions": regions, "region": { "type": ["integer", "object"] }, "kind": { "type": "string" }, "radius": { "type": "number" }, "at": v3, "axis": v3, "depth": { "type": "number" } }, "required": ["op"] } } }, "required": ["edits"] } },
        { "name": "list_edits", "description": "Stage 3's edits in order (yours: by ai, the user's: by hand) and what each did.", "inputSchema": { "type": "object", "properties": {} } },
        { "name": "remove_edit", "description": "Removes one edit (its index from list_edits).", "inputSchema": { "type": "object", "properties": { "index": { "type": "integer" } }, "required": ["index"] } },
        { "name": "clear_edits", "description": "Removes all the edits by ai (default), by hand, or all.", "inputSchema": { "type": "object", "properties": { "by": { "type": "string", "enum": ["ai", "hand", "all"] } } } },
        { "name": "diff_report", "description": "Where a stage is still rougher than the AI mesh: clusters of rough vertices (size, centre, box), largest first. Look at them with capture (focus = centre).", "inputSchema": { "type": "object", "properties": { "stage": stage, "limit": { "type": "integer" } } } }
    ])
}

fn port() -> u16 {
    std::env::var("FORME_RECTIFY_PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(PORT)
}

/// Starts the app next to this program (once).
fn launch(mesh: Option<&str>) -> Result<(), String> {
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = me.parent().ok_or("no folder")?;
    let exe = dir.join(if cfg!(windows) { "forme-rectify.exe" } else { "forme-rectify" });
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

fn connect(start_with: Option<&str>) -> Result<TcpStream, String> {
    if let Ok(s) = TcpStream::connect(("127.0.0.1", port())) {
        return Ok(s);
    }
    launch(start_with)?;
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(30) {
        std::thread::sleep(Duration::from_millis(300));
        if let Ok(s) = TcpStream::connect(("127.0.0.1", port())) {
            return Ok(s);
        }
    }
    Err("the app did not answer (started, but no link after 30 s)".into())
}

/// One request to the app, its answer.
fn call_app(method: &str, params: Value) -> Result<Value, String> {
    let start_with = if method == "open" { params["path"].as_str().map(|s| s.to_string()) } else { None };
    let mut s = connect(start_with.as_deref())?;
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
    json!({ "type": "text", "text": serde_json::to_string_pretty(v).unwrap_or_default() })
}

/// `call_app` on a thread: progress notifications every 5 s (when the
/// client gave a token), "still computing" after the wait limit.
fn call_app_waiting(method: &str, params: Value, progress: Option<&Value>) -> Result<Value, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let (m, p) = (method.to_string(), params);
    std::thread::spawn(move || {
        let _ = tx.send(call_app(&m, p));
    });
    let limit = std::env::var("FORME_RECTIFY_WAIT").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(50);
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
                        "note": format!("the app is still computing after {limit} s and goes on: call status until busy is false, then call {} again (it answers at once when the stage is ready)", if method == "run" { "run_stage" } else { "this tool" }),
                    }));
                }
                if let Some(tok) = progress {
                    n += 1;
                    let mut out = std::io::stdout();
                    let _ = writeln!(out, "{}", json!({ "jsonrpc": "2.0", "method": "notifications/progress", "params": { "progressToken": tok, "progress": n, "message": "computing" } }));
                    let _ = out.flush();
                }
            }
        }
    }
}

fn call_tool(name: &str, args: Value, progress: Option<&Value>) -> Result<Vec<Value>, String> {
    let (method, params) = match name {
        "status" => ("status", json!({})),
        "open_mesh" => ("open", args),
        "run_stage" => ("run", args),
        "capture" => ("capture", args),
        "show" => ("show", args),
        "list_regions" => ("regions", args),
        "pick" => ("pick", args),
        "region_info" => ("region", args),
        "edit_regions" => ("edit", args),
        "list_edits" => ("edits", json!({})),
        "remove_edit" => ("remove_edit", args),
        "clear_edits" => ("clear_edits", args),
        "diff_report" => ("diff", args),
        other => return Err(format!("unknown tool {other}")),
    };
    let mut v = call_app_waiting(method, params, progress)?;
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
                "serverInfo": { "name": "forme-rectify", "version": env!("CARGO_PKG_VERSION") },
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
