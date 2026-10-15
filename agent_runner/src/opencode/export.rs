//! Export of OpenCode sessions into the logs: sub-agents are opaque in the JSONL stream.

use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::Command;
use tracing::{info, warn};

/// Exports the OpenCode sessions named in the per-agent log and their
/// sub-agent sessions. Each export block is appended to the per-agent log,
/// to the shared output log (if any), and to stderr, as soon as it is
/// exported.
pub(crate) fn export_opencode_sessions(
    log_path: &Path,
    output_log_path: Option<&Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    let session_ids = extract_session_ids_from_log(log_path);
    if session_ids.is_empty() {
        info!("No session IDs found in OpenCode log; skipping export");
        return Ok(());
    }

    let mut exported: HashSet<String> = HashSet::new();
    let mut queue: Vec<String> = session_ids;

    while let Some(sid) = queue.pop() {
        if !exported.insert(sid.clone()) {
            continue;
        }
        info!("Exporting OpenCode session {sid}");
        match export_opencode_session(&sid) {
            Ok(json) => {
                let marker = format!("## opencode-export: {sid}\n");
                let block = format!("{marker}{json}\n");

                append_to_file(log_path, &block)?;
                if let Some(out_path) = output_log_path {
                    append_to_file(out_path, &block)?;
                }

                // Write the export block to stderr (same stream as tracing log
                // messages) under a lock so the entire multi-line JSON object is
                // emitted atomically — no log lines can interleave mid-block.
                {
                    use std::io::Write;
                    let stderr = std::io::stderr();
                    let mut handle = stderr.lock();
                    let _ = handle.write_all(block.as_bytes());
                }

                // Discover sub-agent sessions from this export and enqueue them.
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&json) {
                    for child_sid in extract_sub_session_ids_from_export(&parsed) {
                        if !exported.contains(&child_sid) {
                            queue.push(child_sid);
                        }
                    }
                }
            }
            Err(e) => {
                warn!("Failed to export OpenCode session {sid}: {e}");
            }
        }
    }
    Ok(())
}

/// Extract all unique session IDs from an OpenCode JSONL log file.
fn extract_session_ids_from_log(log_path: &Path) -> Vec<String> {
    let Ok(file) = fs::File::open(log_path) else {
        return Vec::new();
    };
    let reader = BufReader::new(file);
    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    for line in reader.lines().map_while(Result::ok) {
        let trimmed = line.trim();
        if !trimmed.starts_with('{') {
            continue;
        }
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed)
            && let Some(sid) = val.get("sessionID").and_then(|v| v.as_str())
            && seen.insert(sid.to_string())
        {
            ids.push(sid.to_string());
        }
    }
    ids
}

/// Export an OpenCode session by ID, returning the raw JSON string.
///
/// Known bug (opencode#14948): `opencode export` truncates JSON when stdout is
/// piped, but works correctly when redirected to a file.  We work around this
/// by redirecting stdout to a temp file instead of capturing it via pipe.
fn export_opencode_session(session_id: &str) -> Result<String, Box<dyn std::error::Error>> {
    let tmp = tempfile::NamedTempFile::new()?;
    let status = Command::new("opencode")
        .args(["export", session_id])
        .stdout(std::fs::File::create(tmp.path())?)
        .status()?;
    if !status.success() {
        return Err(format!("opencode export {session_id} failed (exit {status})").into());
    }
    let stdout = std::fs::read_to_string(tmp.path())?;
    let json_start = stdout.find('{').ok_or("opencode export produced no JSON")?;
    let raw = &stdout[json_start..];

    // Validate the JSON. If it is still broken (e.g. literal control chars
    // inside strings), return an error so the caller can fall back to JSONL.
    serde_json::from_str::<serde_json::Value>(raw)
        .map_err(|e| format!("opencode export {session_id} produced invalid JSON: {e}"))?;

    Ok(raw.to_string())
}

/// Recursively extract sub-agent session IDs from an OpenCode export JSON.
/// Looks for `task` tool_use entries whose `metadata.sessionID` points to a child session.
fn extract_sub_session_ids_from_export(export_json: &serde_json::Value) -> Vec<String> {
    let mut ids = Vec::new();
    let Some(messages) = export_json.get("messages").and_then(|v| v.as_array()) else {
        return ids;
    };
    for msg in messages {
        let Some(parts) = msg.get("parts").and_then(|v| v.as_array()) else {
            continue;
        };
        for part in parts {
            let Some(tool_name) = part.get("tool").and_then(|v| v.as_str()) else {
                continue;
            };
            if tool_name != "task" {
                continue;
            }
            if let Some(sid) = part
                .pointer("/state/metadata/sessionID")
                .or_else(|| part.pointer("/state/metadata/sessionId"))
                .or_else(|| part.pointer("/state/metadata/session_id"))
                .and_then(|v| v.as_str())
                && !sid.is_empty()
            {
                ids.push(sid.to_string());
            }
        }
    }
    ids
}

fn append_to_file(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(text.as_bytes())
}
