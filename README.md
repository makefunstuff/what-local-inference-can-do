# What Local Inference Can Do

I was looking online, and could not find any useful example of what you can achieve with having only local inference nowadays.
This repo is storage of various tasks generated with local models on my hardware, just for the reference, future comparison when new open weights (hopefully) pushed.

## Current Models I have

* qwen 3.8 27b (Qwen3.8-27B-UD-Q5_K_XL.gguf)
* qwen 3.6 27b (Qwen3.6-27B-UD-Q5_K_XL.gguf)
* qwen 3.6 35B A3B (Qwen3.6-35B-A3B-UD-Q5_K_S.gguf)

## Experiments

### 1. heapscan — process heap discovery CLI

* Model: qwen 3.8 27b (Qwen3.8-27B GSQ-RCO IQ3_XXS)
* Code: [heapscan/](heapscan/)

Cross-platform (macOS + Linux) Rust CLI that discovers the heap of a process by PID,
dumps interesting heap objects (strings, pointer arrays, vtable/object candidates) as
JSON for `jq` filtering, and writes custom payloads into specific legitimate heap
objects. `heapscan demo` spawns a dummy target process, runs the full pipeline, and
verifies the write by observing the target's changed output.

```sh
cargo build --release        # in heapscan/
./target/release/heapscan regions <pid> | jq -c '.regions[] | select(.name=="[heap]")'
./target/release/heapscan dump <pid> | jq -r '.objects[] | select(.kind=="string") | .value'
./target/release/heapscan write <pid> <addr> 'Pwned, heap!'
./target/release/heapscan demo
```

**Outcome (verified 2026-09-16):** `heapscan demo` runs the full pipeline
end-to-end and exits 0 — the dummy target's next tick prints
`greeting=Pwned, heap!`, proving the written payload landed in a live
process's heap. `dump` emits valid JSON (74 heap objects on the demo target:
26 strings, 2 vtables, 6 object candidates), and the `write` is observable
on the target's stdout.

Debugging the original "write silently failed" symptom found a real
kernel-contract bug: on Linux, `POKEDATA` was called with the word to write
passed as the *address* argument (glibc forwards `data` untouched, so the
kernel wrote word `0` to the unmapped payload-word address → EIO); the macOS
`PT_READ_D` path had a sibling bug (it returned the return code instead of the
word stored in `data`). Both fixed in [heapscan/src/platform.rs](heapscan/src/platform.rs).

Caveats: the PID must be owned by you (or run as root); on Linux
`/proc/sys/kernel/yama/ptrace_scope=1` blocks attaching to processes you did not
spawn; `dump` stops the target via `ptrace` for the duration of the scan.

### 2. raspberry-lab — 3-RPi isolated homelab (design)

* Model: qwen 3.8 27b (Qwen3.8-27B GSQ-RCO IQ3_XXS, main design); qwen 3.6 35B A3B (Qwen3.6-35B-A3B IQ3_XXS, ansible roles via subagent)
* Code: [raspberry-lab/](raspberry-lab/)

Design-only experiment: an abstract, fully isolated 3-node Raspberry Pi lab
provisioned with Ansible — no internet, no cloud, one flat 10.10.0.0/24.
Three roles: rpi-01 (head: ansible controller, dnsmasq, chrony reference),
rpi-02 (storage: MinIO S3, bucket `lab`), rpi-03 (inference: llama.cpp
`llama-server` running Qwen2.5-1.5B Q4, the local-inference hook into this
repo's theme). A transient staging laptop (10.10.0.2) transports the model
GGUF and the llama.cpp tarball during bootstrap, then is unplugged. The
flashed OS image is preseeded so the lab makes zero apt calls; two
playbooks (`bootstrap.yml`, `deploy-lab.yml`) drive four roles.

```sh
# from rpi-01 (head), in raspberry-lab/ansible/
ansible-playbook playbooks/bootstrap.yml
ansible-playbook playbooks/deploy-lab.yml
```

**Outcome (design verified 2026-09-16):** architecture, network topology,
and the complete ansible stack (inventory, group vars, 4 roles, 2
playbooks, 27 files) are complete and internally consistent against one
frozen variable contract. No hardware was exercised — this is an abstract
lab, so all claims are design decisions, not verified runs.

### 3. softrender — software renderer in C on SDL3

* Model: qwen 3.8 27b (Qwen3.8-27B GSQ-RCO IQ3_XXS)
* Code: [softrender/](softrender/)

CPU rasterizer in C on SDL3: filled triangles (per-vertex color,
barycentric interpolation) and filled rectangles drawn directly into the
window's backing surface — no GPU, no shaders. Demo scene: a rotating
equilateral triangle plus a flat-color triangle orbiting it, deterministic
per frame index. Headless-verified: `make check` renders frame 3 twice at
640x480 with `SDL_VIDEO_DRIVER=dummy` and the dumps are byte-identical; an
independent Python oracle (pure re-implementation, same expression order)
byte-compares its PPM against the C output at 640x480 frame 3 and
320x240 frame 6.

```sh
make                 # in softrender/
./build/softrender   # interactive
SDL_VIDEO_DRIVER=dummy ./build/softrender --frames 4 --dump frame.ppm
```

### 4. clank — minimal unix-style local-inference harness

* Model: qwen 3.8 27b (Qwen3.8-27B GSQ-RCO IQ3_XXS)
* Code: [clank/](clank/)

Minimal Rust CLI that drives a local llama-server (OpenAI-compatible
endpoint, SSE streaming, tool calling) the unix way: prompt and context
arrive via argv/stdin, only data leaves on stdout, breadcrumbs and errors
on stderr. The model may call four read-only filesystem tools
(`read_file`, `list_dir`, `search`, `stat`) — deliberately no shell and no
editing. Context is a tree: piped stdin is one raw-text node, `-c FILE`
loads a saved JSON tree of text/file/nested nodes, so
`rg "userData" src/ | clank -m "what does this do?"` composes like any
pipeline. `--jsonl` emits `tool_call`/`tool_result`/`assistant` events for
`jq`; SIGPIPE is restored, so `clank ... | head` dies cleanly.

```sh
cargo build --release   # in clank/
rg "userData" local/clank/fixtures | ./clank/target/release/clank -m "what does this code do?"
./clank/target/release/clank -c local/clank/fixtures/ctx.json -m "summarize the context"
echo "list the fixtures dir" | ./clank/target/release/clank --jsonl | jq -c
```

**Outcome (verified 2026-09-16):** the piped `rg` example runs the full
tool loop end-to-end — the model called `read_file` on the matched files,
answered on stdout, exit 0. Tree context from a JSON file renders text,
file, and nested nodes in document order; `--jsonl` output is valid
JSONL; `clank -m "..." | head -1` exits 0 with no Rust panic; no prompt
gives exit 2, an unreadable context file exit 1.
