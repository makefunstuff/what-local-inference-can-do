//! heapscan — discover a process heap by PID, dump interesting heap objects
//! as JSON (pipe to `jq`), and write payloads into legitimate heap objects.
//!
//! macOS: `vmmap <pid>` + legacy BSD `ptrace`.
//! Linux:  `/proc/<pid>/maps` + `PTRACE_ATTACH/PEEKDATA/POKEDATA`.

mod demo;
mod json;
mod platform;
mod scan;

use std::env;

fn main() {
    let args: Vec<String> = env::args().collect();
    let code = match args.get(1).map(|s| s.as_str()) {
        Some("regions") => cmd_regions(&args[2..]),
        Some("dump") => cmd_dump(&args[2..]),
        Some("write") => cmd_write(&args[2..]),
        Some("demo") => demo::run(&args[2..]),
        Some(h) if h == "--help" || h == "-h" || h == "help" => {
            print_usage();
            0
        }
        Some(unknown) => {
            eprintln!("error: unknown command '{unknown}'");
            print_usage();
            2
        }
        None => {
            print_usage();
            2
        }
    };
    std::process::exit(code);
}

fn print_usage() {
    eprintln!(
        "heapscan — inspect a process heap as JSON (macOS + Linux)\n\n\
         usage:\n  \
         heapscan regions PID\n  \
         heapscan dump PID [--max-mb N] [--min-str N] [--limit N] [--pretty]\n  \
         heapscan write PID ADDRESS PAYLOAD...\n  \
         heapscan demo [--keep]\n\n\
         notes:\n  \
         * PID must be owned by you (or run as root).\n  \
         * On Linux, /proc/sys/kernel/yama/ptrace_scope=1 blocks attaching to\n  \
         * processes you did not spawn yourself.\n  \
         * `dump` attaches with ptrace, stops the target, scans, and detaches.\n  \
         * `write` only writes to writable, non-executable regions."
    );
}

fn parse_addr(s: &str) -> Result<u64, String> {
    if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(h, 16)
            .map_err(|e| format!("invalid hex address: {s:?} ({e})"))
    } else {
        s.parse::<u64>().map_err(|e| format!("invalid address: {s:?} ({e})"))
    }
}

fn cmd_regions(args: &[String]) -> i32 {
    let pid = match args.first().and_then(|s| s.parse::<i32>().ok()) {
        Some(p) => p,
        None => {
            eprintln!("error: usage: heapscan regions PID");
            return 2;
        }
    };
    match platform::list_regions(pid) {
        Ok(regions) => {
            println!(
                "{}",
                serde_json::to_string(&json::regions_doc(pid, &regions))
                    .expect("fixed schema serializes")
            );
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            3
        }
    }
}

fn cmd_dump(args: &[String]) -> i32 {
    let mut max_mb: u64 = 256;
    let mut min_str: usize = 8;
    let mut limit: usize = 500;
    let mut pretty = false;
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--max-mb" => {
                let v = args.get(i + 1).map(|s| s.as_str()).unwrap_or("");
                match v.parse::<u64>() {
                    Ok(n) => {
                        max_mb = n;
                        i += 2;
                    }
                    Err(_) => {
                        eprintln!("error: --max-mb expects a number");
                        return 2;
                    }
                }
            }
            "--min-str" => {
                let v = args.get(i + 1).map(|s| s.as_str()).unwrap_or("");
                match v.parse::<usize>() {
                    Ok(n) => {
                        min_str = n;
                        i += 2;
                    }
                    Err(_) => {
                        eprintln!("error: --min-str expects a number");
                        return 2;
                    }
                }
            }
            "--limit" => {
                let v = args.get(i + 1).map(|s| s.as_str()).unwrap_or("");
                match v.parse::<usize>() {
                    Ok(n) => {
                        limit = n;
                        i += 2;
                    }
                    Err(_) => {
                        eprintln!("error: --limit expects a number");
                        return 2;
                    }
                }
            }
            "--pretty" => {
                pretty = true;
                i += 1;
            }
            _ => {
                positional.push(args[i].clone());
                i += 1;
            }
        }
    }
    let pid = match positional.first().and_then(|s| s.parse::<i32>().ok()) {
        Some(p) => p,
        None => {
            eprintln!("error: usage: heapscan dump PID [--max-mb N] [--min-str N] [--limit N] [--pretty]");
            return 2;
        }
    };
    let regions = match platform::list_regions(pid) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 3;
        }
    };
    let scan_result = match scan::scan_heap(pid, &regions, max_mb, min_str, limit) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return 3;
        }
    };
    let doc = json::dump_doc(pid, &regions, &scan_result);
    if pretty {
        println!(
            "{}",
            serde_json::to_string_pretty(&doc).expect("fixed schema serializes")
        );
    } else {
        println!("{}", serde_json::to_string(&doc).expect("fixed schema serializes"));
    }
    0
}

fn cmd_write(args: &[String]) -> i32 {
    if args.len() < 3 {
        eprintln!("error: usage: heapscan write PID ADDRESS PAYLOAD...");
        return 2;
    }
    let pid = match args[0].parse::<i32>() {
        Ok(p) => p,
        Err(_) => {
            eprintln!("error: invalid pid: {:?}", args[0]);
            return 2;
        }
    };
    let addr = match parse_addr(&args[1]) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let payload = args[2..].join(" ");
    let regions = match platform::list_regions(pid) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 3;
        }
    };
    let region = match regions
        .iter()
        .find(|r| r.start <= addr && addr + payload.len() as u64 <= r.end)
    {
        Some(r) => r,
        None => {
            eprintln!(
                "error: 0x{addr:x} (+{} bytes) is not fully inside a single mapped region",
                payload.len()
            );
            return 5;
        }
    };
    if !region.perms.contains('w') || region.perms.contains('x') {
        eprintln!(
            "error: refusing to write: region {:?} has perms {:?} (requires writable, non-executable)",
            region.name, region.perms
        );
        return 5;
    }
    match platform::write_region(pid, addr, payload.as_bytes()) {
        Ok(()) => {
            println!(
                "{}",
                serde_json::to_string(&json::WriteResult {
                    address: format!("0x{addr:x}"),
                    written_bytes: payload.len(),
                })
                .expect("fixed schema serializes")
            );
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            3
        }
    }
}
