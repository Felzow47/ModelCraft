//! Software render of an .easm/.eprt (isometric, flat shaded) to a PNG, to check geometry
//! without the GUI: `easmshot in.easm out.png`.
use craft_formats::easm;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let p = std::path::Path::new(&a[1]);
    let b = std::fs::read(p).expect("read");
    let s = easm::load(p, &b).expect("load");
    println!("meshes {} components {} warnings {:?}", s.meshes.len(), s.components.len(), s.warnings);
    let (w, h) = (1400usize, 1000usize);
    // view: isometric-ish, Y up
    let (ya, xa) = (0.7f32, 0.5f32);
    let view = |p: [f32; 3]| {
        let (x, z) = (p[0] * ya.cos() - p[2] * ya.sin(), p[0] * ya.sin() + p[2] * ya.cos());
        let (y, z) = (p[1] * xa.cos() - z * xa.sin(), p[1] * xa.sin() + z * xa.cos());
        [x, y, z]
    };
    let mut tris: Vec<([[f32; 3]; 3], [f32; 3])> = Vec::new();
    for c in &s.components {
        let Some(mi) = c.mesh else { continue };
        let m = &s.meshes[mi];
        let col = c.color.or(m.color).unwrap_or([0.7, 0.7, 0.7, 1.]);
        let wm = c.world;
        let tf = |p: [f32; 3]| [0, 1, 2].map(|r| wm[r] * p[0] + wm[4 + r] * p[1] + wm[8 + r] * p[2] + wm[12 + r]);
        if std::env::var_os("LIST").is_some() {
            let c0 = tf(m.mesh.positions[0]);
            println!("{:>8} tris  at {:?}  {}  (mesh {})", m.mesh.indices.len() / 3, c0, c.name, m.name);
        }
        for t in m.mesh.indices.chunks_exact(3) {
            let v = [0, 1, 2].map(|k| view(tf(m.mesh.positions[t[k] as usize])));
            tris.push((v, [col[0], col[1], col[2]]));
        }
    }
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for (v, _) in &tris {
        for p in v {
            for k in 0..2 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
    }
    let sc = ((w as f32 - 20.) / (hi[0] - lo[0])).min((h as f32 - 20.) / (hi[1] - lo[1]));
    let mut zb = vec![f32::MIN; w * h];
    let mut img = image::RgbImage::from_pixel(w as u32, h as u32, image::Rgb([255, 255, 255]));
    for (v, col) in &tris {
        let q = v.map(|p| [10. + (p[0] - lo[0]) * sc, h as f32 - 10. - (p[1] - lo[1]) * sc, p[2]]);
        let e1 = [v[1][0] - v[0][0], v[1][1] - v[0][1], v[1][2] - v[0][2]];
        let e2 = [v[2][0] - v[0][0], v[2][1] - v[0][1], v[2][2] - v[0][2]];
        let n = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
        let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if l == 0. {
            continue;
        }
        let sh = 0.35 + 0.65 * ((n[2] * 0.8 + n[1] * 0.5) / l).abs().min(1.);
        let px = image::Rgb(col.map(|c| (c * sh * 255.).clamp(0., 255.) as u8));
        let (x0, x1) = (q.iter().map(|p| p[0]).fold(f32::MAX, f32::min).max(0.) as usize, q.iter().map(|p| p[0]).fold(f32::MIN, f32::max).min(w as f32 - 1.) as usize);
        let (y0, y1) = (q.iter().map(|p| p[1]).fold(f32::MAX, f32::min).max(0.) as usize, q.iter().map(|p| p[1]).fold(f32::MIN, f32::max).min(h as f32 - 1.) as usize);
        let area = (q[1][0] - q[0][0]) * (q[2][1] - q[0][1]) - (q[2][0] - q[0][0]) * (q[1][1] - q[0][1]);
        if area.abs() < 1e-12 {
            continue;
        }
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                let w0 = ((q[1][0] - fx) * (q[2][1] - fy) - (q[2][0] - fx) * (q[1][1] - fy)) / area;
                let w1 = ((q[2][0] - fx) * (q[0][1] - fy) - (q[0][0] - fx) * (q[2][1] - fy)) / area;
                let w2 = 1. - w0 - w1;
                if w0 < 0. || w1 < 0. || w2 < 0. {
                    continue;
                }
                let z = w0 * q[0][2] + w1 * q[1][2] + w2 * q[2][2];
                let i = y * w + x;
                if z > zb[i] {
                    zb[i] = z;
                    img.put_pixel(x as u32, y as u32, px);
                }
            }
        }
    }
    img.save(&a[2]).expect("save");
}
