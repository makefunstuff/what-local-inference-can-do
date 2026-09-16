//! End-to-end demo: spawn `demo-target` (a dummy process), dump its heap as
//! JSON, locate a known string object, write a new payload into it, and
//! observe the effect on the target's next tick.

use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::json;
use crate::platform;
use crate::scan;

const GREETING: &str = "Hello, heap!";
const PAYLOAD: &str = "Pwned, heap!";

pub fn run(args: &[String]) -> i32 {
    let keep = args.iter().any(|a| a == "--keep");
    let bin = find_target_bin();
    let mut child = match Command::new(&bin)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: failed to spawn demo target {bin:?}: {e}");
            return 3;
        }
    };
    let pid = child.id() as i32;
    println!("spawned demo-target pid={pid}");

    // Let its heap settle (allocations live across ticks).
    std::thread::sleep(Duration::from_millis(1200));

    let (tx, rx) = mpsc::channel::<String>();
    let child_stdout = child.stdout.take().unwrap();
    let _reader = std::thread::spawn(move || {
        let lines = std::io::BufReader::new(child_stdout).lines();
        for line in lines {
            if line.map(|l| tx.send(l)).is_err() {
                break;
            }
        }
    });

    let regions = match platform::list_regions(pid) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            child.kill().ok();
            return 3;
        }
    };
    let scan_result = match scan::scan_heap(pid, &regions, 256, 8, 500) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            child.kill().ok();
            return 3;
        }
    };
    let doc = json::dump_doc(pid, &regions, &scan_result);
    println!(
        "scanned {} bytes across {} heap region(s); found {} object(s) ({} strings, {} vtables, {} object candidates)",
        doc.stats.bytes_scanned,
        scan_result.regions.len(),
        doc.stats.objects,
        doc.stats.strings,
        doc.stats.vtables,
        doc.stats.object_candidates,
    );

    let greeting_addr = match scan_result
        .objects
        .iter()
        .find(|o| o.kind == "string" && o.value.as_deref() == Some(GREETING))
    {
        Some(o) => o.address,
        None => {
            eprintln!("error: greeting string {GREETING:?} not found in the heap scan");
            child.kill().ok();
            return 5;
        }
    };
    println!("greeting string found at 0x{greeting_addr:x}");
    println!(
        "jq filter: jq '.objects[] | select(.kind==\"string\" and .value==\"{GREETING}\") | .address'"
    );

    match platform::write_region(pid, greeting_addr, PAYLOAD.as_bytes()) {
        Ok(()) => println!("wrote {} bytes to 0x{greeting_addr:x}: {PAYLOAD:?}", PAYLOAD.len()),
        Err(e) => {
            eprintln!("error: {e}");
            child.kill().ok();
            return 3;
        }
    }

    // Wait for the target's next tick to reflect the written payload.
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut found = None;
    loop {
        if Instant::now() > deadline {
            break;
        }
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) if line.contains(PAYLOAD) => {
                found = Some(line);
                break;
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    match found {
        Some(line) => println!("target tick after write: {line}"),
        None => {
            eprintln!("error: target never printed the written payload within 8s");
            child.kill().ok();
            return 6;
        }
    }

    if keep {
        println!("--keep: leaving demo-target running pid={pid}");
    } else {
        child.kill().ok();
        child.wait().ok();
        println!("target stopped");
    }
    0
}

fn find_target_bin() -> PathBuf {
    // Baked in at compile time by cargo when building the heapscan bin.
    if let Some(p) = option_env!("CARGO_BIN_EXE_demo-target") {
        return PathBuf::from(p);
    }
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("demo-target")))
        .unwrap_or_else(|| PathBuf::from("demo-target"))
}
