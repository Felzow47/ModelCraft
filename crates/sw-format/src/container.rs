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

use crate::ctl::{reserve, Ctl, MAX_STREAM};
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

impl StreamInfo {
    /// Uncompressed size when the container records it (SW2015+ records).
    pub fn raw_size(&self) -> Option<u64> {
        match &self.loc {
            Loc::Zip { raw, method, comp, .. } => Some(if *method == 0 { *comp as u64 } else { *raw as u64 }),
            Loc::Ole { .. } => None,
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
        Self::open_ctl(path, &Ctl::default())
    }

    /// [`open`](Self::open) reporting the record scan (which touches the whole file) as byte
    /// progress, with cancellation.
    pub fn open_ctl(path: &Path, ctl: &Ctl) -> Result<Self> {
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
        Self::from_backing_ctl(backing, ctl)
    }

    /// Parse an in-memory file.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        Self::from_backing(Backing::Vec(bytes))
    }

    fn from_backing(data: Backing) -> Result<Self> {
        Self::from_backing_ctl(data, &Ctl::default())
    }

    fn from_backing_ctl(data: Backing, ctl: &Ctl) -> Result<Self> {
        let b = data.bytes();
        if b.len() < 32 {
            return Err(Error::NotSolidWorks("file too small".into()));
        }
        if b[..8] == OLE_MAGIC {
            let streams = list_ole(b)?;
            Ok(Container { kind: ContainerKind::Ole, data, streams })
        } else {
            let streams = list_zip(b, ctl)?;
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
        self.read_info_ctl(s, &Ctl::default(), (0.0, 0.0))
    }

    /// Read and decompress a stream with progress (mapped onto `range` of the overall fraction),
    /// byte counts and cancellation. Memory is reserved fallibly: a stream too large for the
    /// machine gives an error instead of aborting the process.
    pub fn read_info_ctl(&self, s: &StreamInfo, ctl: &Ctl, range: (f32, f32)) -> Result<Vec<u8>> {
        let b = self.data.bytes();
        let stage = "Décompression…";
        let prog = |done: usize, total: usize| {
            let f = if total == 0 { 1.0 } else { done as f32 / total as f32 };
            ctl.report(range.0 + (range.1 - range.0) * f.clamp(0.0, 1.0), stage);
            ctl.bytes(done as u64, total as u64);
        };
        match &s.loc {
            Loc::Zip { data_off, comp, raw, method } => {
                let src = b
                    .get(*data_off..data_off + comp)
                    .ok_or_else(|| Error::Corrupt(format!("{}: record out of bounds", s.name)))?;
                match method {
                    0 => {
                        let mut v = Vec::new();
                        reserve(&mut v, src.len(), &s.name)?;
                        v.extend_from_slice(src);
                        Ok(v)
                    }
                    _ => {
                        if *raw == 0 {
                            return Ok(Vec::new());
                        }
                        // a size field close to 4 GiB may be a ZIP64 placeholder: do not trust it
                        let limit = if *raw as u64 >= 0xFFFF_0000 { MAX_STREAM } else { (*raw as u64 + 1024).min(MAX_STREAM) };
                        inflate(src, false, (*raw).min(limit as usize), limit, ctl, &prog).map_err(|e| match e {
                            Error::Corrupt(m) => Error::Corrupt(format!("{}: {m}", s.name)),
                            e => e,
                        })
                    }
                }
            }
            Loc::Ole { path, zlb } => {
                let mut cf = cfb::CompoundFile::open(Cursor::new(b))
                    .map_err(|e| Error::Corrupt(format!("OLE: {e}")))?;
                let mut st = cf.open_stream(path).map_err(|e| Error::Corrupt(format!("{path}: {e}")))?;
                let len = st.len() as usize;
                let mut raw = Vec::new();
                reserve(&mut raw, len, &s.name)?;
                ctl.report(range.0, stage);
                st.read_to_end(&mut raw)?;
                ctl.check()?;
                if *zlb {
                    decode_zlb_ctl(&raw, ctl, &prog)?.ok_or_else(|| Error::Corrupt(format!("{}: bad __ZLB payload", s.name)))
                } else {
                    Ok(raw)
                }
            }
        }
    }

    /// Decompress a stream piece by piece into `sink` without keeping it in memory (SW2015+
    /// ZIP records; other layouts are read whole then handed over). Returns the raw size.
    pub fn stream_info(&self, s: &StreamInfo, ctl: &Ctl, range: (f32, f32), stage: &str, sink: &mut dyn FnMut(&[u8]) -> Result<()>) -> Result<u64> {
        let b = self.data.bytes();
        let prog = |done: usize, total: usize| {
            let f = if total == 0 { 1.0 } else { done as f32 / total as f32 };
            ctl.report(range.0 + (range.1 - range.0) * f.clamp(0.0, 1.0), stage);
            ctl.bytes(done as u64, total as u64);
        };
        match &s.loc {
            Loc::Zip { data_off, comp, raw, method } => {
                let src = b
                    .get(*data_off..data_off + comp)
                    .ok_or_else(|| Error::Corrupt(format!("{}: record out of bounds", s.name)))?;
                if *method == 0 {
                    for (k, c) in src.chunks(4 << 20).enumerate() {
                        sink(c)?;
                        prog(k * (4 << 20) + c.len(), src.len());
                        ctl.check()?;
                    }
                    return Ok(src.len() as u64);
                }
                if *raw == 0 {
                    return Ok(0);
                }
                // streaming: no output buffer, so allow up to 64 GiB of decompressed data
                inflate_into(src, false, 64 << 30, ctl, &prog, sink).map_err(|e| match e {
                    Error::Corrupt(m) => Error::Corrupt(format!("{}: {m}", s.name)),
                    e => e,
                })
            }
            Loc::Ole { path, zlb: true } => {
                // keep only the compressed stream; inflate the `__ZLB` blocks one at a time
                let mut cf = cfb::CompoundFile::open(Cursor::new(b)).map_err(|e| Error::Corrupt(format!("OLE: {e}")))?;
                let mut st = cf.open_stream(path).map_err(|e| Error::Corrupt(format!("{path}: {e}")))?;
                let mut raw = Vec::new();
                reserve(&mut raw, st.len() as usize, &s.name)?;
                st.read_to_end(&mut raw)?;
                if let Some(start) = memchr::memmem::find(&raw[..raw.len().min(64)], &ZLB_GUID).map(|i| i + 16) {
                    let mut i = start;
                    let mut total = 0u64;
                    while i + 8 <= raw.len() {
                        let rs = u32::from_le_bytes(raw[i..i + 4].try_into().unwrap()) as usize;
                        let cs = u32::from_le_bytes(raw[i + 4..i + 8].try_into().unwrap()) as usize;
                        if cs == 0 || i + 8 + cs > raw.len() {
                            break;
                        }
                        let Ok(v) = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&raw[i + 8..i + 8 + cs], rs.max(1) + 4096) else { break };
                        total += v.len() as u64;
                        sink(&v)?;
                        i += 8 + cs;
                        prog(i, raw.len());
                        ctl.check()?;
                    }
                    if total > 0 {
                        return Ok(total);
                    }
                }
                drop(raw);
                let v = self.read_info_ctl(s, ctl, range)?;
                sink(&v)?;
                Ok(v.len() as u64)
            }
            Loc::Ole { .. } => {
                let v = self.read_info_ctl(s, ctl, range)?;
                sink(&v)?;
                Ok(v.len() as u64)
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

/// Streaming inflate (raw deflate or zlib) into a fallibly grown buffer, with progress and
/// cancellation every few megabytes. `limit` caps the output size.
fn inflate(src: &[u8], zlib: bool, expect: usize, limit: u64, ctl: &Ctl, prog: &dyn Fn(usize, usize)) -> Result<Vec<u8>> {
    let mut out: Vec<u8> = Vec::new();
    reserve(&mut out, expect, "décompression")?;
    inflate_into(src, zlib, limit, ctl, prog, &mut |chunk| {
        if out.capacity() - out.len() < chunk.len() {
            // grow by at least 25 % to keep the number of reallocations low
            let room = (limit as usize).saturating_sub(out.len()).max(chunk.len());
            let extra = chunk.len().max(out.len() / 4).min(room);
            reserve(&mut out, extra, "décompression")?;
        }
        out.extend_from_slice(chunk);
        Ok(())
    })?;
    Ok(out)
}

/// Streaming inflate handing each decompressed piece (≤ 4 MiB) to `sink`; nothing else is
/// kept in memory. Returns the number of bytes produced.
fn inflate_into(
    src: &[u8],
    zlib: bool,
    limit: u64,
    ctl: &Ctl,
    prog: &dyn Fn(usize, usize),
    sink: &mut dyn FnMut(&[u8]) -> Result<()>,
) -> Result<u64> {
    use miniz_oxide::inflate::stream::{inflate as step, InflateState};
    use miniz_oxide::{DataFormat, MZError, MZFlush, MZStatus};
    let mut st = InflateState::new_boxed(if zlib { DataFormat::Zlib } else { DataFormat::Raw });
    let mut buf = vec![0u8; 4 << 20];
    let mut pos = 0usize;
    let mut last = 0usize;
    let mut total = 0u64;
    loop {
        let r = step(&mut st, &src[pos..], &mut buf, MZFlush::None);
        pos += r.bytes_consumed;
        if r.bytes_written > 0 {
            if total + r.bytes_written as u64 > limit {
                return Err(Error::TooLarge(format!("flux décompressé supérieur à {} Mo", limit >> 20)));
            }
            total += r.bytes_written as u64;
            sink(&buf[..r.bytes_written])?;
        }
        match r.status {
            Ok(MZStatus::StreamEnd) => break,
            Ok(_) => {}
            Err(MZError::Buf) if pos >= src.len() => break, // truncated: keep what we have
            Err(MZError::Buf) => {}
            Err(e) => {
                if total == 0 {
                    return Err(Error::Corrupt(format!("deflate {e:?}")));
                }
                break;
            }
        }
        if r.bytes_consumed == 0 && r.bytes_written == 0 {
            break;
        }
        if pos - last >= 4 << 20 {
            last = pos;
            prog(pos, src.len());
            ctl.check()?;
        }
    }
    prog(src.len(), src.len());
    Ok(total)
}

/// Decode a `__ZLB` payload: GUID, then `[u32 raw][u32 comp][zlib]` blocks.
/// Falls back to a plain zlib stream after a short header.
pub fn decode_zlb(raw: &[u8]) -> Option<Vec<u8>> {
    decode_zlb_ctl(raw, &Ctl::default(), &|_, _| {}).ok().flatten()
}

fn decode_zlb_ctl(raw: &[u8], ctl: &Ctl, prog: &dyn Fn(usize, usize)) -> Result<Option<Vec<u8>>> {
    if raw.is_empty() {
        return Ok(Some(Vec::new()));
    }
    let start = memchr::memmem::find(&raw[..raw.len().min(64)], &ZLB_GUID).map(|i| i + 16);
    if let Some(mut i) = start {
        let mut out: Vec<u8> = Vec::new();
        let mut ok = false;
        let mut last = 0usize;
        while i + 8 <= raw.len() {
            let rs = u32::from_le_bytes(raw[i..i + 4].try_into().unwrap()) as usize;
            let cs = u32::from_le_bytes(raw[i + 4..i + 8].try_into().unwrap()) as usize;
            if cs == 0 || i + 8 + cs > raw.len() {
                break;
            }
            if out.len() as u64 + rs as u64 > MAX_STREAM {
                return Err(Error::TooLarge(format!("flux décompressé supérieur à {} Mo", MAX_STREAM >> 20)));
            }
            match miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&raw[i + 8..i + 8 + cs], rs.max(1) + 4096) {
                Ok(v) => {
                    if out.capacity() - out.len() < v.len() {
                        let extra = v.len().max(out.len() / 4);
                        reserve(&mut out, extra, "décompression")?;
                    }
                    out.extend_from_slice(&v);
                    ok = true;
                }
                Err(_) => break,
            }
            i += 8 + cs;
            if i - last >= 4 << 20 {
                last = i;
                prog(i, raw.len());
                ctl.check()?;
            }
        }
        if ok {
            return Ok(Some(out));
        }
    }
    // Fallback: try zlib at small offsets.
    for off in 0..raw.len().min(48) {
        if let Ok(v) = inflate(&raw[off..], true, 0, MAX_STREAM, ctl, prog) {
            if !v.is_empty() {
                return Ok(Some(v));
            }
        }
        ctl.check()?;
    }
    Ok(None)
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

fn list_zip(b: &[u8], ctl: &Ctl) -> Result<Vec<StreamInfo>> {
    // Collect every candidate local header. Files also contain decoy records and
    // a disguised central directory, so we do not skip over data spans; the real
    // records are the ones sharing the (per-file) 4-byte signature.
    let mut cands: Vec<([u8; 4], StreamInfo)> = Vec::new();
    let finder = memchr::memmem::Finder::new(&ZIP_SIG);
    let mut pos = 4usize;
    let mut next_report = 32usize << 20;
    while pos < b.len() {
        let Some(rel) = finder.find(&b[pos..]) else { break };
        let p = pos + rel;
        pos = p + 1;
        if p >= next_report {
            next_report = p + (32 << 20);
            ctl.bytes(p as u64, b.len() as u64);
            ctl.check()?;
        }
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
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let want = best.filter(|(_, n)| *n >= 2).map(|(m, _)| m);
    for (m, s) in cands {
        if want.is_none_or(|w| w == m) && seen.insert(s.name.clone()) {
            out.push(s);
        }
    }
    ctl.bytes(b.len() as u64, b.len() as u64);
    Ok(out)
}
