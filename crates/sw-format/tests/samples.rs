//! Integration tests on real SolidWorks files.
//! Set SW_SAMPLES=/path/to/folder containing p1..p4.SLDPRT, a1.SLDASM, a2.SLDASM.
//! Tests are skipped (pass trivially) when the variable or files are absent.

use std::path::PathBuf;
use sw_format::{load_scene, ContainerKind, DocKind, LoadOptions};

fn sample(name: &str) -> Option<PathBuf> {
    let dir = std::env::var_os("SW_SAMPLES")?;
    let p = PathBuf::from(dir).join(name);
    if p.exists() {
        Some(p)
    } else {
        eprintln!("skip: {} absent", p.display());
        None
    }
}

fn check_part(name: &str, kind: ContainerKind, min_faces: usize, min_tris: usize) {
    let Some(p) = sample(name) else { return };
    let s = load_scene(&p, &LoadOptions::default()).expect("load");
    assert_eq!(s.kind, DocKind::Part);
    assert_eq!(s.container, kind);
    assert!(s.has_geometry());
    let faces: usize = s.meshes.iter().map(|m| m.mesh.face_count).sum();
    assert!(faces >= min_faces, "{name}: {faces} faces");
    assert!(
        s.drawn_triangles() >= min_tris,
        "{name}: {} tris",
        s.drawn_triangles()
    );
    for m in &s.meshes {
        let n = m.mesh.positions.len() as u32;
        assert!(m.mesh.indices.iter().all(|&i| i < n));
    }
}

#[test]
fn p1_sw2015() {
    check_part("p1.SLDPRT", ContainerKind::Sw2015, 9, 160);
}

#[test]
fn p2_ole() {
    check_part("p2.SLDPRT", ContainerKind::Ole, 13, 1600);
}

#[test]
fn p3_ole_no_preview() {
    check_part("p3.SLDPRT", ContainerKind::Ole, 18, 8000);
}

#[test]
fn p4_ole() {
    check_part("p4.SLDPRT", ContainerKind::Ole, 148, 5000);
    let Some(p) = sample("p4.SLDPRT") else { return };
    let s = load_scene(&p, &LoadOptions::default()).unwrap();
    let (lo, hi) = s.bounds().unwrap();
    assert!(
        (hi[0] - lo[0] - 1.304).abs() < 0.01,
        "length {}",
        hi[0] - lo[0]
    );
}

#[test]
fn a1_assembly_sw2015() {
    let Some(p) = sample("a1.SLDASM") else { return };
    let s = load_scene(&p, &LoadOptions::default()).unwrap();
    assert_eq!(s.kind, DocKind::Assembly);
    assert_eq!(s.container, ContainerKind::Sw2015);
    assert_eq!(s.meshes.len(), 2);
    assert_eq!(s.missing_components(), 0);
    assert!(s.drawn_triangles() >= 5000);
}

#[test]
fn a2_nested_assembly_transforms() {
    let Some(p) = sample("a2.SLDASM") else { return };
    let s = load_scene(&p, &LoadOptions::default()).unwrap();
    assert_eq!(s.kind, DocKind::Assembly);
    assert_eq!(s.container, ContainerKind::Ole);
    assert_eq!(s.missing_components(), 0);
    assert!(
        s.components.iter().any(|c| c.depth >= 2),
        "nested subassemblies expected"
    );
    // the two latch subassemblies sit left and right of the grille (x ≈ ±0.249 m)
    let xs: Vec<f32> = s
        .components
        .iter()
        .filter(|c| c.is_assembly && c.depth == 0)
        .map(|c| c.world[12])
        .collect();
    assert!(xs.iter().any(|&x| (x - 0.249).abs() < 0.01), "{xs:?}");
    assert!(xs.iter().any(|&x| (x + 0.249).abs() < 0.01), "{xs:?}");
    assert!(s.drawn_triangles() >= 70_000);
}

/// The streaming decoder (stream fed in small pieces) must find exactly the faces of the
/// in-memory decoder, with the same document attribution.
#[test]
fn streaming_decoder_matches_in_memory() {
    use sw_format::container::Container;
    use sw_format::dl;
    for name in [
        "p1.SLDPRT",
        "p2.SLDPRT",
        "p3.SLDPRT",
        "p4.SLDPRT",
        "a1.SLDASM",
        "a2.SLDASM",
    ] {
        let Some(p) = sample(name) else { continue };
        let c = Container::open(&p).unwrap();
        let Some(d) = c.read("Contents/DisplayLists").unwrap() else {
            continue;
        };
        let reference = dl::decode(&d);
        let want: Vec<(Option<String>, usize, usize)> = reference
            .blocks
            .iter()
            .flat_map(|b| {
                b.faces
                    .iter()
                    .map(move |f| (b.key(), f.offset, f.positions.len()))
            })
            .collect();
        for piece in [4099usize, 65_536, 1 << 20] {
            let mut got: Vec<(Option<String>, usize, usize)> = Vec::new();
            let ctl = sw_format::Ctl::default();
            let mut visit = |path: Option<&str>, f: dl::FacePatch| -> sw_format::Result<()> {
                got.push((path.map(dl::file_key), f.offset, f.positions.len()));
                Ok(())
            };
            let mut sd = dl::StreamDecoder::new(&ctl, &mut visit);
            for ch in d.chunks(piece) {
                sd.feed(ch).unwrap();
            }
            sd.finish().unwrap();
            // in-memory blocks are grouped by document: compare as sorted sets
            let mut a = want.clone();
            a.sort();
            got.sort();
            assert_eq!(a, got, "{name} piece {piece}");
        }
        // full pipeline, forced streaming, gives the same geometry
        let normal = load_scene(&p, &LoadOptions::default()).unwrap();
        std::env::set_var("MODELCRAFT_STREAM", "1");
        let streamed = load_scene(&p, &LoadOptions::default()).unwrap();
        std::env::remove_var("MODELCRAFT_STREAM");
        assert_eq!(
            normal.drawn_triangles(),
            streamed.drawn_triangles(),
            "{name}"
        );
        assert!(streamed.timings.dl.as_ref().is_some_and(|s| s.streamed));
    }
}
