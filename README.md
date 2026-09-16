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

Caveats: the PID must be owned by you (or run as root); on Linux
`/proc/sys/kernel/yama/ptrace_scope=1` blocks attaching to processes you did not
spawn; `dump` stops the target via `ptrace` for the duration of the scan.
