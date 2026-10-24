//! SolidWorks file containers.
//!
//! * Pre-2015 files are OLE Compound Files (`D0 CF 11 E0`). Streams whose name
//!   ends in `__ZLB` are compressed: a 16-byte GUID followed by blocks of
//!   `[u32 raw size][u32 compressed size][zlib stream]`.
//! * SW2015+ files are a sequence of ZIP-like local file records whose
//!   `PK\3\4` signature is replaced by `EA B9 23 87`, with nibble-swapped
//!   names and raw-deflate data.
//!
//! The file is memory-mapped read-only; nothing is ever written.

use crate::{Error, Result};
use memmap2::Mmap;
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::Path;

const OLE_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
/// GUID that prefixes compressed (`__ZLB`) payloads.
pub const ZLB_GUID: [u8; 16] = [
    0x23, 0x1d, 0xd5, 0x71, 0xda, 0x81, 0x48, 0xa2, 0xa8, 0x58, 0x98, 0xb2, 0x1b, 0x89, 0xef, 0x99,
];
/// version=0x14, flags=6, method=8 : start of a disguised local header (after the 4-byte magic).
const ZIP_SIG: [u8; 6] = [0x14, 0x00, 0x06, 0x00, 0x08, 0x00];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerKind {
    /// OLE compound file (SolidWorks <= 2014)
    Ole,
    /// Disguised ZIP container (SolidWorks 2015+)
    Sw2015,
    /// Not a SolidWorks container: another file format (label is the format name).
    Other(&'static str),
}

impl ContainerKind {
    pub fn label(self) -> &'static str {
        match self {
            ContainerKind::Ole => "OLE (<2015)",
            ContainerKind::Sw2015 => "SW2015+",
            ContainerKind::Other(s) => s,
        }
    }
}

#[derive(Debug, Clone)]
enum Loc {
    /// OLE stream path (as given to cfb), plus whether it is `__ZLB` compressed.
    Ole { path: String, zlb: bool },
    /// Disguised ZIP record.
    Zip { data_off: usize, comp: usize, raw: usize, method: u16 },
}

/// A stream inside the container (logical name, without `__ZLB` suffix).
#[derive(Debug, Clone)]
pub struct StreamInfo {
    pub name: String,
    /// Stored (possibly compressed) size in bytes.
    pub stored_size: u64,
    loc: Loc,
}

enum Backing {
    Map(Mmap),
    Vec(Vec<u8>),
}

impl Backing {
    fn bytes(&self) -> &[u8] {
        match self {
            Backing::Map(m) => m,
            Backing::Vec(v) => v,
        }
    }
}

/// An opened SolidWorks container (read-only).
pub struct Container {
    kind: ContainerKind,
    data: Backing,
    streams: Vec<StreamInfo>,
}

impl Container {
    /// Open a file strictly read-only (memory-mapped).
    pub fn open(path: &Path) -> Result<Self> {
        let f = File::open(path)?;
        let len = f.metadata()?.len();
        let backing = if len == 0 {
            Backing::Vec(Vec::new())
        } else {
            // SAFETY: read-only mapping; the file is not modified by us. If another
            // process truncates it concurrently we may fault, which is accepted for a viewer.
            match unsafe { Mmap::map(&f) } {
                Ok(m) => Backing::Map(m),
                Err(_) => {
                    let mut v = Vec::with_capacity(len as usize);
                    let mut f = f;
                    f.read_to_end(&mut v)?;
                    Backing::Vec(v)
                }
            }
        };
        Self::from_backing(backing)
    }

    /// Parse an in-memory file.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        Self::from_backing(Backing::Vec(bytes))
    }

    fn from_backing(data: Backing) -> Result<Self> {
        let b = data.bytes();
        if b.len() < 32 {
            return Err(Error::NotSolidWorks("file too small".into()));
        }
        if b[..8] == OLE_MAGIC {
            let streams = list_ole(b)?;
            Ok(Container { kind: ContainerKind::Ole, data, streams })
        } else {
            let streams = list_zip(b);
            if streams.is_empty() {
                return Err(Error::NotSolidWorks("neither OLE nor SW2015+ container".into()));
            }
            Ok(Container { kind: ContainerKind::Sw2015, data, streams })
        }
    }

    pub fn kind(&self) -> ContainerKind {
        self.kind
    }

    pub fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }

    pub fn file_size(&self) -> usize {
        self.data.bytes().len()
    }

    /// Find a stream by logical name (case-insensitive, `/` separated).
    pub fn find(&self, name: &str) -> Option<&StreamInfo> {
        self.streams.iter().find(|s| s.name.eq_ignore_ascii_case(name))
    }

    pub fn has(&self, name: &str) -> bool {
        self.find(name).is_some()
    }

    /// Read and decompress a stream by logical name.
    pub fn read(&self, name: &str) -> Result<Option<Vec<u8>>> {
        match self.find(name) {
            None => Ok(None),
            Some(s) => self.read_info(s).map(Some),
        }
    }

    pub fn read_info(&self, s: &StreamInfo) -> Result<Vec<u8>> {
        let b = self.data.bytes();
        match &s.loc {
            Loc::Zip { data_off, comp, raw, method } => {
                let src = b
                    .get(*data_off..data_off + comp)
                    .ok_or_else(|| Error::Corrupt(format!("{}: record out of bounds", s.name)))?;
                match method {
                    0 => Ok(src.to_vec()),
                    _ => {
                        if *raw == 0 {
                            return Ok(Vec::new());
                        }
                        let out = miniz_oxide::inflate::decompress_to_vec_with_limit(src, raw + 1024)
                            .map_err(|e| Error::Corrupt(format!("{}: deflate {:?}", s.name, e.status)))?;
                        Ok(out)
                    }
                }
            }
            Loc::Ole { path, zlb } => {
                let mut cf = cfb::CompoundFile::open(Cursor::new(b))
                    .map_err(|e| Error::Corrupt(format!("OLE: {e}")))?;
                let mut st = cf.open_stream(path).map_err(|e| Error::Corrupt(format!("{path}: {e}")))?;
                let mut raw = Vec::new();
                st.read_to_end(&mut raw)?;
                if *zlb {
                    decode_zlb(&raw).ok_or_else(|| Error::Corrupt(format!("{}: bad __ZLB payload", s.name)))
                } else {
                    Ok(raw)
                }
            }
        }
    }
}

fn list_ole(b: &[u8]) -> Result<Vec<StreamInfo>> {
    let cf = cfb::CompoundFile::open(Cursor::new(b)).map_err(|e| Error::Corrupt(format!("OLE: {e}")))?;
    let mut out: Vec<StreamInfo> = Vec::new();
    for e in cf.walk() {
        if !e.is_stream() {
            continue;
        }
        let path = e.path().to_string_lossy().replace('\\', "/");
        let logical_full = path.trim_start_matches('/').to_string();
        let (logical, zlb) = if let Some(stripped) = logical_full.strip_suffix("__ZLB") {
            (stripped.to_string(), true)
        } else if let Some(stripped) = logical_full.strip_suffix("__Zip") {
            (stripped.to_string(), true)
        } else {
            (logical_full.clone(), false)
        };
        // Preference order when both exist: `name__ZLB`, then `name`, then `name__Zip`.
        let rank = |p: &str| if p.ends_with("__ZLB") { 0 } else if p.ends_with("__Zip") { 2 } else { 1 };
        if let Some(existing) = out.iter_mut().find(|s| s.name == logical) {
            let old_rank = match &existing.loc { Loc::Ole { path, .. } => rank(path), _ => 3 };
            if rank(&path) < old_rank {
                *existing = StreamInfo { name: logical, stored_size: e.len(), loc: Loc::Ole { path, zlb } };
            }
            continue;
        }
        out.push(StreamInfo { name: logical, stored_size: e.len(), loc: Loc::Ole { path, zlb } });
    }
    Ok(out)
}

/// Decode a `__ZLB` payload: GUID, then `[u32 raw][u32 comp][zlib]` blocks.
/// Falls back to a plain zlib stream after a short header.
pub fn decode_zlb(raw: &[u8]) -> Option<Vec<u8>> {
    if raw.is_empty() {
        return Some(Vec::new());
    }
    let start = memchr::memmem::find(&raw[..raw.len().min(64)], &ZLB_GUID).map(|i| i + 16);
    if let Some(mut i) = start {
        let mut out = Vec::new();
        let mut ok = false;
        while i + 8 <= raw.len() {
            let rs = u32::from_le_bytes(raw[i..i + 4].try_into().ok()?) as usize;
            let cs = u32::from_le_bytes(raw[i + 4..i + 8].try_into().ok()?) as usize;
            if cs == 0 || i + 8 + cs > raw.len() {
                break;
            }
            match miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&raw[i + 8..i + 8 + cs], rs.max(1) + 4096) {
                Ok(v) => {
                    out.extend_from_slice(&v);
                    ok = true;
                }
                Err(_) => break,
            }
            i += 8 + cs;
        }
        if ok {
            return Some(out);
        }
    }
    // Fallback: try zlib / raw deflate at small offsets.
    for off in 0..raw.len().min(48) {
        if let Ok(v) = miniz_oxide::inflate::decompress_to_vec_zlib(&raw[off..]) {
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

fn nibble_swap(b: &[u8]) -> String {
    b.iter().map(|&x| ((x >> 4) | (x << 4)) as char).collect()
}

fn rd_u16(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}
fn rd_u32(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

fn list_zip(b: &[u8]) -> Vec<StreamInfo> {
    // Collect every candidate local header. Files also contain decoy records and
    // a disguised central directory, so we do not skip over data spans; the real
    // records are the ones sharing the (per-file) 4-byte signature.
    let mut cands: Vec<([u8; 4], StreamInfo)> = Vec::new();
    let finder = memchr::memmem::Finder::new(&ZIP_SIG);
    let mut pos = 4usize;
    while pos < b.len() {
        let Some(rel) = finder.find(&b[pos..]) else { break };
        let p = pos + rel;
        pos = p + 1;
        let (Some(_crc), Some(cs), Some(us), Some(nl), Some(xl)) =
            (rd_u32(b, p + 10), rd_u32(b, p + 14), rd_u32(b, p + 18), rd_u16(b, p + 22), rd_u16(b, p + 24))
        else {
            continue;
        };
        let (cs, us, nl, xl) = (cs as usize, us as usize, nl as usize, xl as usize);
        if nl == 0 || nl > 512 || cs > b.len() {
            continue;
        }
        let name_off = p + 26;
        let data_off = name_off + nl + xl;
        if data_off + cs > b.len() {
            continue;
        }
        let name = nibble_swap(&b[name_off..name_off + nl]);
        if !name.chars().all(|c| (' '..='~').contains(&c) || (c as u32) >= 0xA0) {
            continue;
        }
        let method = rd_u16(b, p + 4).unwrap_or(8);
        let magic: [u8; 4] = b[p - 4..p].try_into().unwrap();
        cands.push((magic, StreamInfo { name, stored_size: cs as u64, loc: Loc::Zip { data_off, comp: cs, raw: us, method } }));
    }
    // Most frequent signature = the file's record magic.
    let mut counts: std::collections::HashMap<[u8; 4], usize> = std::collections::HashMap::new();
    for (m, _) in &cands {
        *counts.entry(*m).or_default() += 1;
    }
    let best = counts.iter().max_by_key(|(_, c)| **c).map(|(m, c)| (*m, *c));
    let mut out: Vec<StreamInfo> = Vec::new();
    match best {
        Some((magic, n)) if n >= 2 => {
            for (m, s) in cands {
                if m == magic && !out.iter().any(|o| o.name == s.name) {
                    out.push(s);
                }
            }
        }
        _ => {
            for (_, s) in cands {
                if !out.iter().any(|o| o.name == s.name) {
                    out.push(s);
                }
            }
        }
    }
    out
}
