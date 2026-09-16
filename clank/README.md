# clank — minimal unix-style local-inference harness

A small Rust CLI that talks to a local llama-server (OpenAI-compatible
endpoint with tool calling) the unix way: prompt and context come in via
argv/stdin, data goes out on stdout, breadcrumbs and errors on stderr.

```sh
cargo build --release   # in clank/
./target/release/clank [OPTIONS] [PROMPT...]
```

## Usage

```sh
# plain prompt
clank -m "reply with exactly: pong"

# pipe context in, ask a question (the pipe is the context)
rg "userData" src/ | clank -m "what does this code do?"

# context from a saved tree file (JSON, paths relative to cwd)
clank -c ctx.json -m "summarize the context"

# JSONL events on stdout for jq
echo "list the fixtures dir" | clank --jsonl | jq -c
```

## Unix contract

- **stdout = data only**: final assistant text, or `--jsonl` events
  (`tool_call`, `tool_result`, `assistant`). Nothing else.
- **stderr = diagnostics**: tool breadcrumbs (`> read_file ...`,
  `< read_file ok (N B)`) and errors.
- **prompt**: `-m TEXT`, positional text, or piped stdin (stdin is the
  prompt only when no other prompt is given).
- **context**: piped stdin becomes a raw-text context node when a prompt is
  also given; `-c FILE` loads a saved context (JSON tree or plain text) and
  takes precedence over stdin.
- **SIGPIPE restored** (Rust sets SIG_IGN by default), so `clank ... | head`
  dies cleanly.
- **line-buffered, per-delta flush** in text mode.
- **exit codes**: `0` ok, `1` failure (model/server/IO), `2` usage error.
- **NO_COLOR** honored; `--quiet` suppresses stderr breadcrumbs.

## Context tree

A context file is either plain text (one text node) or a JSON tree:

```json
[
  { "text": "raw text node" },
  { "file": "path/relative/to/cwd" },
  { "children": [ { "file": "a.md" }, { "text": "more" } ] }
]
```

Nodes render in document order; file leaves are read at render time and
emitted as `─── path ───` + content. Piped stdin is always a single raw-text
node, so `rg "x" | clank -m "..."` composes like any unix pipeline.

## Tools (deliberately read-only)

The model can call four filesystem observers and nothing else — no shell,
no editing:

| tool | description |
|---|---|
| `read_file(path, start_line?, end_line?)` | numbered file content |
| `list_dir(path)` | directory entries (name, type, size) |
| `search(pattern, path?, glob?)` | regex search, `file:line: text` output |
| `stat(path)` | file/directory metadata |

Tool errors (missing path, bad args) are returned to the model as data, not
crashes; a bad context file or a server error is a process failure (exit 1).

## Configuration

Flags override `$CLANK_*` environment variables, which override built-in
defaults:

| flag | env | default |
|---|---|---|
| `-m` / positional | — | — |
| `-c` / `--context` | — | — |
| `--model` | `CLANK_MODEL` | `qwen3.8-27b-gsq-rco-iq3xxs` |
| `--base-url` | `CLANK_BASE_URL` | `http://127.0.0.1:40583/v1` |
| `--api-key` | `CLANK_API_KEY` | none |
| `--timeout` | `CLANK_TIMEOUT` | 600 s per call |
| `--max-tokens` | — | 8192 |
| `--max-rounds` | — | 12 tool rounds |
| `--jsonl`, `-q`/`--quiet`, `-h`/`--help` | — | — |

## Verified

* Model: qwen 3.8 27b (Qwen3.8-27B GSQ-RCO IQ3_XXS) on llama-server at
  `http://127.0.0.1:40583/v1` (tool-calling via the OpenAI-compatible
  endpoint, SSE streaming).
* `clank -m "reply with exactly: pong"` → `pong` on stdout, exit 0.
* `rg "userData" local/clank/fixtures | clank -m "what does this code do?"`
  → the model called `read_file` on the matched files (breadcrumbs on
  stderr) and answered on stdout, exit 0.
* Tree context: `clank -c local/clank/fixtures/ctx.json -m "..."` renders
  text + file + nested nodes in document order, exit 0.
* `--jsonl` emits `tool_call` / `tool_result` / `assistant` events parseable
  by `jq`, exit 0.
* `clank -m "..." | head -1` → exit 0 (SIGPIPE handled, no Rust panic).
* No prompt with empty stdin → exit 2; unreadable context file → exit 1;
  `--help` → exit 0.
