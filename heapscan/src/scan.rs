//! Heuristic heap-object discovery over raw heap bytes.
//!
//! Labels are candidates, not ground truth:
//!   * `string`           - printable ASCII runs of at least `min_str` bytes
//!   * `ptr_array`        - 3+ consecutive words that are all valid pointers
//!   * `vtable_candidate` - a ptr_array whose every target sits in a code (x) region
//!   * `object_candidate` - an object with a field pointing at a known vtable
//!     (C++ vtable-first objects and Rust `Box<dyn Trait>` fat pointers)

use std::collections::HashMap;

use crate::platform::{is_heap_region, Region, Tracee};

#[derive(Debug, Clone)]
pub struct Field {
    pub index: usize,
    pub value: u64,
    pub target: &'static str,
}

#[derive(Debug, Clone)]
pub struct Object {
    pub address: u64,
    pub kind: &'static str,
    pub length: Option<usize>,
    pub value: Option<String>,
    pub count: Option<usize>,
    pub pointers: Vec<u64>,
    pub targets: Vec<&'static str>,
    pub fields: Vec<Field>,
}

pub struct ScanResult {
    pub objects: Vec<Object>,
    pub bytes_scanned: u64,
    pub read_errors: u64,
    pub regions: Vec<(String, u64, u64)>,
}

pub fn scan_heap(
    pid: i32,
    regions: &[Region],
    max_mb: u64,
    min_str: usize,
    limit: usize,
) -> Result<ScanResult, String> {
    let max_bytes = max_mb * 1024 * 1024;
    let heap: Vec<&Region> = regions
        .iter()
        .filter(|r| is_heap_region(r) && r.end > r.start && (r.end - r.start) <= max_bytes)
        .collect();
    if heap.is_empty() {
        return Err(format!(
            "no writable heap region found for pid {pid} (expected a '[heap]' or anonymous rw region)"
        ));
    }

    let mut tracee = Tracee::attach(pid)?;
    let mut result = ScanResult {
        objects: Vec::new(),
        bytes_scanned: 0,
        read_errors: 0,
        regions: Vec::new(),
    };

    for r in &heap {
        let (objs, bytes, errs) = scan_region(&tracee, r, regions, min_str)?;
        result.regions.push((r.name.clone(), r.start, r.end));
        result.bytes_scanned += bytes;
        result.read_errors += errs;
        for o in objs {
            result.objects.push(o);
            if result.objects.len() >= limit {
                break;
            }
        }
        if result.objects.len() >= limit {
            break;
        }
    }

    tracee.detach()?;
    Ok(result)
}

fn scan_region(
    tracee: &Tracee,
    region: &Region,
    all: &[Region],
    min_str: usize,
) -> Result<(Vec<Object>, u64, u64), String> {
    let len = (region.end - region.start) as usize;
    let mut buf = vec![0u8; len];
    let mut read_errors = 0u64;
    for off in (0..len).step_by(8) {
        match tracee.read_u64(region.start + off as u64) {
            Ok(w) => buf[off..off + 8].copy_from_slice(&w.to_le_bytes()),
            Err(_) => read_errors += 1, // hole: leave zero-filled
        }
    }
    Ok((scan_bytes(&buf, region, all, min_str), len as u64, read_errors))
}

fn scan_bytes(buf: &[u8], region: &Region, regions: &[Region], min_str: usize) -> Vec<Object> {
    let base = region.start;
    let mut out: Vec<Object> = Vec::new();
    let mut string_counts: HashMap<String, u32> = HashMap::new();

    // 1) printable runs -> strings (deduped by content, max 3 per content)
    let mut i = 0usize;
    while i < buf.len() {
        if (32..=126).contains(&buf[i]) {
            let start = i;
            while i < buf.len() && (32..=126).contains(&buf[i]) {
                i += 1;
            }
            let l = i - start;
            if l >= min_str {
                let value = String::from_utf8_lossy(&buf[start..i]).into_owned();
                let seen = string_counts.entry(value.clone()).or_insert(0);
                if *seen < 3 {
                    *seen += 1;
                    out.push(Object {
                        address: base + start as u64,
                        kind: "string",
                        length: Some(l),
                        value: Some(value),
                        count: None,
                        pointers: Vec::new(),
                        targets: Vec::new(),
                        fields: Vec::new(),
                    });
                }
            }
        } else {
            i += 1;
        }
    }

    // 2) windows of 3+ consecutive valid pointers -> ptr_array / vtable_candidate
    let mut vtables: Vec<u64> = Vec::new();
    let mut i = 0usize;
    while i + 8 <= buf.len() {
        let w = u64::from_le_bytes(buf[i..i + 8].try_into().unwrap());
        if !is_ptr(w, regions) {
            i += 8;
            continue;
        }
        let mut j = i;
        while j + 8 <= buf.len() && (j - i) / 8 < 64 {
            let w2 = u64::from_le_bytes(buf[j..j + 8].try_into().unwrap());
            if is_ptr(w2, regions) {
                j += 8;
            } else {
                break;
            }
        }
        let count = (j - i) / 8;
        if count >= 3 {
            let pointers: Vec<u64> = (0..count)
                .map(|k| u64::from_le_bytes(buf[i + k * 8..i + k * 8 + 8].try_into().unwrap()))
                .collect();
            let targets: Vec<&'static str> = pointers.iter().map(|p| classify(*p, regions)).collect();
            let all_code = targets.iter().all(|t| *t == "code");
            if all_code {
                for p in &pointers {
                    vtables.push(*p);
                }
            }
            out.push(Object {
                address: base + i as u64,
                kind: if all_code { "vtable_candidate" } else { "ptr_array" },
                length: None,
                value: None,
                count: Some(count),
                pointers,
                targets,
                fields: Vec::new(),
            });
            i = j;
        } else {
            i += 8;
        }
    }

    // 3) object candidates: any of the first 4 words points at a known vtable.
    //    If the word before the vtable is a valid pointer, the object starts
    //    there (Rust `Box<dyn Trait>`: [payload][vtable]); otherwise the vtable
    //    word is the object start (C++-style vtable-first).
    let mut i = 0usize;
    let mut reported: std::collections::HashSet<u64> = std::collections::HashSet::new();
    while i + 8 <= buf.len() {
        let mut hit = None;
        for k in 0..4 {
            if i + k * 8 + 8 <= buf.len() {
                let v = u64::from_le_bytes(buf[i + k * 8..i + k * 8 + 8].try_into().unwrap());
                if vtables.contains(&v) {
                    hit = Some(k);
                    break;
                }
            }
        }
        if let Some(k) = hit {
            let mut k_eff = k;
            if k > 0 {
                let prev = u64::from_le_bytes(
                    buf[i + (k - 1) * 8..i + (k - 1) * 8 + 8].try_into().unwrap(),
                );
                if is_ptr(prev, regions) {
                    k_eff = k - 1;
                }
            }
            let obj_off = i + k_eff * 8;
            let addr = base + obj_off as u64;
            if reported.insert(addr) {
                let mut fields: Vec<Field> = Vec::new();
                for kk in 0..4 {
                    if obj_off + kk * 8 + 8 <= buf.len() {
                        let v =
                            u64::from_le_bytes(buf[obj_off + kk * 8..obj_off + kk * 8 + 8].try_into().unwrap());
                        let target = if vtables.contains(&v) { "vtable" } else { classify(v, regions) };
                        fields.push(Field { index: kk, value: v, target });
                    }
                }
                out.push(Object {
                    address: addr,
                    kind: "object_candidate",
                    length: None,
                    value: None,
                    count: None,
                    pointers: Vec::new(),
                    targets: Vec::new(),
                    fields,
                });
            }
        }
        i += 8;
    }

    out
}

fn is_ptr(addr: u64, regions: &[Region]) -> bool {
    addr != 0 && regions.iter().any(|r| r.start <= addr && addr < r.end)
}

fn classify(addr: u64, regions: &[Region]) -> &'static str {
    match regions.iter().find(|r| r.start <= addr && addr < r.end) {
        Some(r) => {
            if r.perms.contains('x') {
                "code"
            } else if crate::platform::is_heap_region(r) {
                "heap"
            } else if r.name == "[stack]" || r.name == "Stack" {
                "stack"
            } else {
                "data"
            }
        }
        None => "unmapped",
    }
}
