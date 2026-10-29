//! Write a synthetic SW2015+-style part with a huge DisplayLists stream, to test that very large
//! files load with bounded memory (and fail cleanly when they cannot).
//!
//!     cargo run --release -p sw-format --example synth_big -- out.SLDPRT 300
//!
//! The second argument is the approximate number of millions of vertices (24 bytes each raw;
//! the noise added to the coordinates keeps the deflated size close to the raw size).

use std::io::Write;

fn arr_hdr(out: &mut Vec<u8>, es: u32, tag: u32, n: u32) {
    for v in [es, tag, 2, n] {
        out.extend_from_slice(&v.to_le_bytes());
    }
}

fn record(out: &mut Vec<u8>, magic: [u8; 4], name: &str, data: &[u8], raw: usize) {
    let swapped: Vec<u8> = name.bytes().map(|x| x.rotate_left(4)).collect();
    out.extend_from_slice(&magic);
    out.extend_from_slice(&[0x14, 0x00, 0x06, 0x00, 0x08, 0x00]); // version, flags, method=8
    out.extend_from_slice(&[0u8; 4]); // time/date
    out.extend_from_slice(&0u32.to_le_bytes()); // crc
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&(raw as u32).to_le_bytes());
    out.extend_from_slice(&(swapped.len() as u16).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&swapped);
    out.extend_from_slice(data);
}

fn main() {
    let mut a = std::env::args().skip(1);
    let path = a.next().expect("output path");
    let mverts: f64 = a.next().and_then(|s| s.parse().ok()).unwrap_or(20.0);
    let total = (mverts * 1e6) as usize;
    const W: usize = 500; // ribbon: one strip of 2*W vertices per face
    let faces = total / (2 * W);
    eprintln!("{faces} faces, {} vertices", faces * 2 * W);
    let mut dl: Vec<u8> = Vec::with_capacity(faces * (2 * W * 24 + 64) + 64);
    dl.extend_from_slice(b"synthetic display list\0\0");
    let mut seed = 0x1234_5678u32;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed as f32 / u32::MAX as f32) - 0.5
    };
    let side = (faces as f64).sqrt().ceil() as usize;
    for f in 0..faces {
        let (fx, fz) = ((f % side) as f32, (f / side) as f32);
        arr_hdr(&mut dl, 4, 0x65, 1);
        dl.extend_from_slice(&((2 * W) as u32).to_le_bytes());
        arr_hdr(&mut dl, 12, 0x64, (2 * W) as u32);
        let mut nrm = Vec::with_capacity(2 * W * 12);
        for i in 0..W {
            for r in 0..2 {
                let x = fx * 0.05 + i as f32 * 0.0001;
                let z = fz * 0.002 + r as f32 * 0.0019;
                let y = (x * 20.0).sin() * 0.01 + (z * 13.0).cos() * 0.01 + rnd() * 1e-5;
                for v in [x, y, z] {
                    dl.extend_from_slice(&v.to_le_bytes());
                }
                for v in [rnd() * 0.01, 1.0f32, rnd() * 0.01] {
                    nrm.extend_from_slice(&v.to_le_bytes());
                }
            }
        }
        arr_hdr(&mut dl, 12, 0x66, (2 * W) as u32);
        dl.extend_from_slice(&nrm);
        dl.extend_from_slice(&[0u8; 8]);
    }
    eprintln!("raw DisplayLists: {} MB, deflating…", dl.len() >> 20);
    let comp = miniz_oxide::deflate::compress_to_vec(&dl, 1);
    eprintln!("deflated: {} MB", comp.len() >> 20);
    let raw = dl.len();
    drop(dl);
    let magic = [0x31, 0x9a, 0x47, 0x2c];
    let mut out: Vec<u8> = Vec::with_capacity(comp.len() + 4096);
    let small = miniz_oxide::deflate::compress_to_vec(b"<synthetic/>", 1);
    record(&mut out, magic, "Header2", &small, 12);
    record(&mut out, magic, "Contents/DisplayLists", &comp, raw);
    record(&mut out, magic, "Contents/Config-0", &small, 12);
    std::fs::File::create(&path).and_then(|mut f| f.write_all(&out)).expect("write");
    eprintln!("wrote {path}: {} MB", out.len() >> 20);
}
