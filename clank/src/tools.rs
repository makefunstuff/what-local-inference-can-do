//! Read-only filesystem tools exposed to the model.
//!
//! Deliberately no shell, no editing, no deletion: the harness only observes.
//! Output formats are unix-flavored and jq-friendly where they are structured.

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

pub const SYSTEM_PROMPT: &str = "\
You are `clank`, a minimal unix-style inference harness. You talk to the user through stdout.
You can observe the local filesystem with these read-only tools (no shell, no editing):
- read_file(path, start_line?, end_line?) — read a text file; numbered lines
- list_dir(path) — list directory entries
- search(pattern, path?) — regex search under a file or directory; rg-style `file:line: text`
- stat(path) — metadata for a path (JSON)
Rules:
- You only observe: you never write, delete, or run commands.
- Cite `file:line` when you refer to code.
- Be concise.
";

pub fn definitions() -> Value {
    json!([{
        "type": "function",
        "function": {
            "name": "read_file",
            "description": "Read a text file. Returns numbered lines; use start_line/end_line (1-based, inclusive) for large files.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path relative to cwd." },
                    "start_line": { "type": "integer", "minimum": 1 },
                    "end_line": { "type": "integer", "minimum": 1 }
                },
                "required": ["path"]
            }
        }
    }, {
        "type": "function",
        "function": {
            "name": "list_dir",
            "description": "List directory entries (name, d/-, size).",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Directory path relative to cwd; '.' for cwd." }
                },
                "required": ["path"]
            }
        }
    }, {
        "type": "function",
        "function": {
            "name": "search",
            "description": "Regex search over text files under a file or directory. Output: one `file:line: text` per match (capped).",
            "parameters": {
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Rust regex pattern." },
                    "path": { "type": "string", "description": "File or directory to search; default '.'." }
                },
                "required": ["pattern"]
            }
        }
    }, {
        "type": "function",
        "function": {
            "name": "stat",
            "description": "Stat a path. Returns JSON: path, type, size, mtime (unix seconds), mode.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path relative to cwd." }
                },
                "required": ["path"]
            }
        }
    }])
}

/// Execute a tool. Returns (output, ok). Errors are returned as data, so the
/// model can react to them; only server-level failures kill the process.
pub fn execute(name: &str, args: &Value) -> (String, bool) {
    let res = match name {
        "read_file" => read_file(args),
        "list_dir" => list_dir(args),
        "search" => search(args),
        "stat" => stat(args),
        other => Err(format!("unknown tool: {other}")),
    };
    match res {
        Ok(out) => (out, true),
        Err(e) => (format!("error: {e}"), false),
    }
}

fn need<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

const DEFAULT_READ_LINES: usize = 400;

fn read_file(args: &Value) -> Result<String, String> {
    let path = need(args, "path").ok_or_else(|| "missing required arg: path".to_string())?;
    let text = fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let lines: Vec<&str> = text.split('\n').collect();
    let total = lines.len();
    let start = args.get("start_line").and_then(|v| v.as_u64()).unwrap_or(1).max(1) as usize;
    let end = args
        .get("end_line")
        .and_then(|v| v.as_u64())
        .map(|e| (e as usize).min(total))
        .unwrap_or(total.min(DEFAULT_READ_LINES));
    let end = end.max(start - 1).min(total);
    if start > total {
        return Ok(format!("{path}: file has {total} lines; start_line {start} is out of range"));
    }
    let mut out = format!("{path} ({total} lines, showing {start}..{end})\n");
    for (n, line) in lines.iter().enumerate().take(end).skip(start - 1) {
        out.push_str(&format!("{:4} | {}\n", n + 1, line));
    }
    if end < total {
        out.push_str(&format!("... ({} more lines; use start_line/end_line)\n", total - end));
    }
    Ok(out)
}

fn list_dir(args: &Value) -> Result<String, String> {
    let path = need(args, "path").ok_or_else(|| "missing required arg: path".to_string())?;
    let dir = Path::new(path);
    if !dir.is_dir() {
        return Err(format!("{path}: not a directory"));
    }
    let mut entries: Vec<(String, bool, u64)> = fs::read_dir(dir)
        .map_err(|e| format!("cannot list {path}: {e}"))?
        .filter_map(|e| e.ok())
        .map(|e| {
            let md = e.metadata().ok();
            (
                e.file_name().to_string_lossy().into_owned(),
                md.as_ref().map(|m| m.is_dir()).unwrap_or(false),
                md.map(|m| m.len()).unwrap_or(0),
            )
        })
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = format!("{path}/\n");
    for (name, is_dir, size) in entries {
        out.push_str(&if is_dir {
            format!("  d  {name}/\n")
        } else {
            format!("  -  {name} ({size} B)\n")
        });
    }
    Ok(out)
}

const SEARCH_CAP: usize = 500;
const MAX_FILE_BYTES: u64 = 4_000_000;

fn search(args: &Value) -> Result<String, String> {
    let pattern = need(args, "pattern")
        .ok_or_else(|| "missing required arg: pattern".to_string())?;
    let re = regex::Regex::new(pattern).map_err(|e| format!("bad regex {pattern:?}: {e}"))?;
    let path = need(args, "path").unwrap_or(".");
    let root = Path::new(path);

    let mut out = String::new();
    let mut count = 0usize;
    let mut truncated = false;
    if root.is_file() {
        walk_one(root, &re, &mut out, &mut count, &mut truncated);
    } else if root.is_dir() {
        walk_dir(root, &re, &mut out, &mut count, &mut truncated);
    } else {
        return Err(format!("{path}: no such file or directory"));
    }
    if count == 0 {
        out = "no matches\n".to_string();
    }
    if truncated {
        out.push_str("... (truncated at 500 matches)\n");
    }
    Ok(out)
}

fn is_binary(b: &[u8]) -> bool {
    b.iter().take(8000).any(|&b| b == 0)
}

fn walk_one(
    p: &Path,
    re: &regex::Regex,
    out: &mut String,
    count: &mut usize,
    truncated: &mut bool,
) {
    let Ok(meta) = fs::metadata(p) else { return };
    if meta.len() > MAX_FILE_BYTES {
        return;
    }
    let Ok(bytes) = fs::read(p) else { return };
    if is_binary(&bytes) {
        return;
    }
    let text = String::from_utf8_lossy(&bytes);
    for (n, line) in text.lines().enumerate() {
        if re.is_match(line) {
            out.push_str(&format!("{}:{}: {}\n", p.display(), n + 1, line.trim_end()));
            *count += 1;
            if *count >= SEARCH_CAP {
                *truncated = true;
                return;
            }
        }
    }
}

fn walk_dir(
    dir: &Path,
    re: &regex::Regex,
    out: &mut String,
    count: &mut usize,
    truncated: &mut bool,
) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut files: Vec<PathBuf> = Vec::new();
    let mut dirs: Vec<PathBuf> = Vec::new();
    for e in entries.filter_map(|e| e.ok()) {
        let p = e.path();
        let ft = e.file_type();
        if ft.as_ref().map(|t| t.is_symlink()).unwrap_or(false) {
            continue;
        }
        if ft.as_ref().map(|t| t.is_dir()).unwrap_or(false) {
            dirs.push(p);
        } else if p.is_file() {
            files.push(p);
        }
    }
    files.sort();
    for p in files {
        if *truncated {
            return;
        }
        walk_one(&p, re, out, count, truncated);
    }
    dirs.sort();
    for d in dirs {
        if *truncated {
            return;
        }
        walk_dir(&d, re, out, count, truncated);
    }
}

fn stat(args: &Value) -> Result<String, String> {
    let path = need(args, "path").ok_or_else(|| "missing required arg: path".to_string())?;
    let md = fs::metadata(path).map_err(|e| format!("cannot stat {path}: {e}"))?;
    let kind = if md.is_dir() {
        "dir"
    } else if md.is_symlink() {
        "symlink"
    } else {
        "file"
    };
    let mtime = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut v = json!({
        "path": path,
        "type": kind,
        "size": md.len(),
        "mtime": mtime,
    });
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Some(obj) = v.as_object_mut() {
            obj.insert("mode".into(), Value::String(format!("{:04o}", md.permissions().mode())));
        }
    }
    Ok(v.to_string())
}

/// Compact one-line rendering of tool args for stderr breadcrumbs.
pub fn compact_args(v: &Value) -> String {
    match v {
        Value::Object(m) => m
            .iter()
            .map(|(k, val)| format!("{k}={}", short(val)))
            .collect::<Vec<_>>()
            .join(" "),
        other => other.to_string(),
    }
}

fn short(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "-".into(),
        other => format!("…{}chars", other.to_string().len()),
    }
}
