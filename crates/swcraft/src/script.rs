//! Scripted states used for documentation screenshots (`--show measure`).

use crate::document::{Document, FaceSel};
use crate::measure::{self, Measure};
use crate::settings::Settings;
use glam::Vec3;

/// Add a hole diameter, an overall distance and the bounding box to the document.
pub fn demo_measures(d: &mut Document, _st: &Settings) {
    let Some(s) = &d.scene else { return };
    let mut best: Option<(f32, usize, usize, usize, measure::Cylinder)> = None;
    let mut extreme: Option<(Vec3, Vec3)> = None;
    for (ci, c) in s.components.iter().enumerate() {
        let Some(mi) = c.mesh else { continue };
        if !d.visible.get(ci).copied().unwrap_or(true) {
            continue;
        }
        let m = &s.meshes[mi].mesh;
        let w = d.display[ci];
        for f in 0..m.face_count {
            let tris = m.face_triangles(f);
            if tris.len() < 8 {
                continue;
            }
            if let Some(cyl) = measure::fit_cylinder(m, tris, w) {
                if cyl.rel_err < 0.02 && cyl.sweep > 5.5 {
                    // prefer holes, then the biggest
                    let score = cyl.radius * if cyl.inward { 10.0 } else { 1.0 };
                    if best.as_ref().is_none_or(|b| score > b.0) {
                        best = Some((score, ci, mi, f, cyl));
                    }
                }
            }
        }
        for p in &m.positions {
            let p = w.transform_point3(Vec3::from(*p));
            extreme = Some(match extreme {
                None => (p, p),
                Some((a, b)) => (if p.x < a.x { p } else { a }, if p.x > b.x { p } else { b }),
            });
        }
    }
    if let Some((a, b)) = extreme {
        d.measure.results.push(Measure::Distance { a, b });
    }
    if let Some((_, ci, mi, f, cyl)) = best {
        let m = &s.meshes[mi].mesh;
        let tri = m.face_triangles(f).start;
        let at = cyl.center + cyl.axis.any_orthonormal_vector() * cyl.radius;
        d.face_sel = Some(FaceSel { comp: ci, mesh: mi, face: Some(f), tri });
        d.measure.results.push(Measure::Diameter { cyl, at });
        d.highlight_dirty = true;
    }
    d.show_bbox = true;
    d.instances_dirty = true;
}
