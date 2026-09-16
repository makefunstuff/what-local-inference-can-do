//! clank — minimal unix-style local-inference harness.
//!
//! Unix contract:
//! - prompt: positional text, `-m TEXT`, or piped stdin
//! - context: piped stdin (when a prompt is also given), or a JSON tree file via `-c`
//! - stdout = data only (assistant text, or `--jsonl` events); stderr = breadcrumbs/errors
//! - SIGPIPE restored, so `clank ... | head` dies like any unix tool
//! - exit codes: 0 ok, 1 failure (model/server/IO), 2 usage
//!
//! Tools are read-only filesystem observers (read_file, list_dir, search, stat).
//! There is deliberately no shell and no editing tool.

mod client;
mod context;
mod tools;

use clap::Parser;
use serde_json::{json, Value};
use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;
use std::time::Duration;

const DEFAULT_MODEL: &str = "qwen3.8-27b-gsq-rco-iq3xxs";
const DEFAULT_BASE_URL: &str = "http://127.0.0.1:40583/v1";

#[derive(Debug)]
pub enum Fail {
    /// usage/config error -> exit 2
    Usage(String),
    /// model/server failure -> exit 1
    Model(String),
    /// filesystem/IO failure -> exit 1
    IO(String),
}

impl Fail {
    pub fn code(&self) -> i32 {
        match self {
            Fail::Usage(_) => 2,
            Fail::Model(_) | Fail::IO(_) => 1,
        }
    }
    pub fn msg(&self) -> &str {
        match self {
            Fail::Usage(m) | Fail::Model(m) | Fail::IO(m) => m,
        }
    }
}

#[derive(Parser)]
#[command(
    name = "clank",
    version,
    about = "Minimal unix-style local-inference harness (stdin/stdout, read-only tools)"
)]
struct Args {
    /// prompt text; when omitted, positional args (or piped stdin) form the prompt
    #[arg(short = 'm', long)]
    message: Option<String>,

    /// prompt text (space-joined) when -m is omitted
    #[arg(trailing_var_arg = true)]
    prompt: Vec<String>,

    /// context file: JSON tree or plain text (paths relative to cwd)
    #[arg(short = 'c', long)]
    context: Option<PathBuf>,

    /// emit JSONL events on stdout instead of raw text
    #[arg(short = 'j', long)]
    jsonl: bool,

    /// suppress stderr breadcrumbs
    #[arg(short = 'q', long)]
    quiet: bool,

    /// model name
    #[arg(long, env = "CLANK_MODEL", default_value = DEFAULT_MODEL)]
    model: String,

    /// OpenAI-compatible base URL
    #[arg(long, env = "CLANK_BASE_URL", default_value = DEFAULT_BASE_URL)]
    base_url: String,

    /// API key (local servers usually need none)
    #[arg(long, env = "CLANK_API_KEY")]
    api_key: Option<String>,

    /// per-request timeout, seconds
    #[arg(long, value_name = "SECS", default_value_t = 600)]
    timeout: u64,

    /// maximum tool-call rounds
    #[arg(long, value_name = "N", default_value_t = 12)]
    max_rounds: usize,

    /// maximum completion tokens
    #[arg(long, value_name = "N", default_value_t = 8192)]
    max_tokens: u32,
}

fn main() {
    #[cfg(unix)]
    unsafe {
        // Rust installs SIG_IGN for SIGPIPE; restore the default disposition so
        // `clank ... | head` terminates like a normal unix tool, not a panic.
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    let args = Args::parse();
    std::process::exit(run(args));
}

fn run(args: Args) -> i32 {
    let fail = |e: Fail| -> i32 {
        let _ = writeln!(std::io::stderr(), "clank: {}", e.msg());
        e.code()
    };

    // stdin: non-tty means content. It is the prompt when no other prompt is
    // given, otherwise it is a context text node.
    let stdin = if std::io::stdin().is_terminal() {
        None
    } else {
        let mut buf = String::new();
        match std::io::stdin().read_to_string(&mut buf) {
            Ok(_) => Some(buf),
            Err(e) => return fail(Fail::Usage(format!("reading stdin: {e}"))),
        }
    };

    let prompt = args
        .message
        .clone()
        .or_else(|| (!args.prompt.is_empty()).then(|| args.prompt.join(" ")))
        .or_else(|| stdin.as_deref().filter(|s| !s.trim().is_empty()).map(str::to_string));

    let Some(prompt) = prompt.filter(|p| !p.trim().is_empty()) else {
        return fail(Fail::Usage("no prompt: pass -m TEXT, positional text, or pipe stdin".into()));
    };

    // context: -c file, or piped stdin when a prompt is present.
    let mut tree: Option<context::Tree> = None;
    if let Some(path) = &args.context {
        match context::load_file(path) {
            Ok(t) => tree = Some(t),
            Err(e) => return fail(e),
        }
    }
    if tree.is_none() {
        if let Some(s) = stdin {
            if !s.trim().is_empty() {
                tree = Some(context::parse(&s).expect("stdin always parses"));
            }
        }
    }

    let mut messages: Vec<Value> =
        vec![json!({ "role": "system", "content": tools::SYSTEM_PROMPT })];
    if let Some(t) = tree {
        messages.push(json!({
            "role": "user",
            "content": format!("Context (tree, document order):\n\n{}", t.render()),
        }));
    }
    messages.push(json!({ "role": "user", "content": prompt }));
    let config = ureq::Agent::config_builder()
        .timeout_per_call(Some(Duration::from_secs(args.timeout)))
        .http_status_as_error(false)
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let tools_json = tools::definitions();

    let mut rounds = 0usize;
    loop {
        let mut emit = |delta: &str| {
            if !args.jsonl {
                let _ = print!("{delta}");
                let _ = std::io::stdout().flush();
            }
        };

        let result = match client::stream_round(
            &agent,
            &args.base_url,
            &args.model,
            args.api_key.as_deref(),
            args.max_tokens,
            &messages,
            &tools_json,
            &mut emit,
            std::env::var("CLANK_DEBUG").ok().as_deref(),
        ) {
            Ok(r) => r,
            Err(e) => return fail(e),
        };

        if result.tool_calls.is_empty() {
            if args.jsonl {
                let _ = println!("{}", json!({ "type": "assistant", "content": result.text }));
            }
            return 0;
        }

        rounds += 1;
        if rounds > args.max_rounds {
            return fail(Fail::Model(format!(
                "stopped after {} tool rounds (--max-rounds)",
                args.max_rounds
            )));
        }

        let mut assistant_tcs = Vec::new();
        for (i, tc) in result.tool_calls.iter().enumerate() {
            let id = tc.id.clone().unwrap_or_else(|| format!("call_{rounds}_{i}"));
            assistant_tcs.push(json!({
                "id": id,
                "type": "function",
                "function": { "name": tc.name, "arguments": tc.arguments },
            }));
        }

        let content = if result.text.is_empty() {
            Value::Null
        } else {
            Value::String(result.text.clone())
        };
        // assistant message first, then its tool results (OpenAI message order).
        messages.push(json!({ "role": "assistant", "content": content, "tool_calls": assistant_tcs }));

        for (i, tc) in result.tool_calls.iter().enumerate() {
            let id = tc.id.clone().unwrap_or_else(|| format!("call_{rounds}_{i}"));
            let args_v: Value =
                serde_json::from_str(&tc.arguments).unwrap_or(Value::Null);
            if args.jsonl {
                let _ = println!("{}", json!({ "type": "tool_call", "name": tc.name, "arguments": args_v }));
            } else if !args.quiet {
                let _ = eprintln!("> {} {}", tc.name, tools::compact_args(&args_v));
            }

            let (out, ok) = tools::execute(&tc.name, &args_v);
            if args.jsonl {
                let _ = println!("{}", json!({ "type": "tool_result", "name": tc.name, "ok": ok, "output": out }));
            } else if !args.quiet {
                let _ = eprintln!(
                    "< {} {} ({} B)",
                    tc.name,
                    if ok { "ok" } else { "err" },
                    out.len()
                );
            }

            messages.push(json!({ "role": "tool", "tool_call_id": id, "content": out }));
        }
    }
}
