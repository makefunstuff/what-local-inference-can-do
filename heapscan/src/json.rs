//! JSON output via serde.

use serde::Serialize;

use crate::platform::Region;
use crate::scan::ScanResult;

fn hex(v: u64) -> String {
    format!("0x{v:x}")
}

#[derive(Serialize)]
pub struct RegionOut {
    pub name: String,
    pub start: String,
    pub end: String,
    pub size: u64,
    pub perms: String,
}

#[derive(Serialize)]
pub struct ScannedRegionOut {
    pub name: String,
    pub start: String,
    pub end: String,
}

#[derive(Serialize)]
pub struct FieldOut {
    pub index: usize,
    pub value: String,
    pub target: &'static str,
}

#[derive(Serialize)]
pub struct ObjectOut {
    pub address: String,
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub length: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<usize>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pointers: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<FieldOut>,
}

#[derive(Serialize)]
pub struct Stats {
    pub objects: usize,
    pub strings: usize,
    pub ptr_arrays: usize,
    pub vtables: usize,
    pub object_candidates: usize,
    pub bytes_scanned: u64,
    pub read_errors: u64,
}

#[derive(Serialize)]
pub struct Dump {
    pub pid: i32,
    pub regions: Vec<RegionOut>,
    pub scanned_regions: Vec<ScannedRegionOut>,
    pub objects: Vec<ObjectOut>,
    pub stats: Stats,
}

#[derive(Serialize)]
pub struct RegionsDoc {
    pub pid: i32,
    pub regions: Vec<RegionOut>,
}

#[derive(Serialize)]
pub struct WriteResult {
    pub address: String,
    pub written_bytes: usize,
}

pub fn region_out(r: &Region) -> RegionOut {
    RegionOut {
        name: r.name.clone(),
        start: hex(r.start),
        end: hex(r.end),
        size: r.end - r.start,
        perms: r.perms.clone(),
    }
}

pub fn regions_doc(pid: i32, regions: &[Region]) -> RegionsDoc {
    RegionsDoc {
        pid,
        regions: regions.iter().map(region_out).collect(),
    }
}

pub fn dump_doc(pid: i32, regions: &[Region], scan: &ScanResult) -> Dump {
    let objects: Vec<ObjectOut> = scan
        .objects
        .iter()
        .map(|o| {
            ObjectOut {
                address: hex(o.address),
                kind: o.kind,
                length: o.length,
                value: o.value.clone(),
                count: o.count,
                pointers: o.pointers.iter().map(|p| hex(*p)).collect(),
                targets: o.targets.iter().map(|t| t.to_string()).collect(),
                fields: o
                    .fields
                    .iter()
                    .map(|f| FieldOut {
                        index: f.index,
                        value: hex(f.value),
                        target: f.target,
                    })
                    .collect(),
            }
        })
        .collect();
    let stats = Stats {
        objects: objects.len(),
        strings: scan.objects.iter().filter(|o| o.kind == "string").count(),
        ptr_arrays: scan.objects.iter().filter(|o| o.kind == "ptr_array").count(),
        vtables: scan
            .objects
            .iter()
            .filter(|o| o.kind == "vtable_candidate")
            .count(),
        object_candidates: scan
            .objects
            .iter()
            .filter(|o| o.kind == "object_candidate")
            .count(),
        bytes_scanned: scan.bytes_scanned,
        read_errors: scan.read_errors,
    };
    Dump {
        pid,
        regions: regions.iter().map(region_out).collect(),
        scanned_regions: scan
            .regions
            .iter()
            .map(|(name, start, end)| ScannedRegionOut {
                name: name.clone(),
                start: hex(*start),
                end: hex(*end),
            })
            .collect(),
        objects,
        stats,
    }
}
