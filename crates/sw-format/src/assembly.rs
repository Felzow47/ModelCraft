//! Assembly structure from `swXmlContents/COMPINSTANCETREE` (XML).
//!
//! `swFile` = referenced documents, `swModel` = (document, configuration) with
//! child `swReference` component instances. Each reference carries a 4x4
//! `swTransform` in row-vector convention (translation in the last row), which
//! read column-major gives the usual column-vector matrix.

use glam::DMat4;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct AsmFile {
    pub path: String,
    pub is_assembly: bool,
}

#[derive(Debug, Clone)]
pub struct AsmReference {
    pub name: String,
    pub model: u32,
    pub transform: DMat4,
    pub suppressed: bool,
    pub hidden: bool,
}

#[derive(Debug, Clone)]
pub struct AsmModel {
    pub name: String,
    pub config: String,
    pub file: Option<u32>,
    pub refs: Vec<AsmReference>,
}

#[derive(Debug, Clone, Default)]
pub struct AsmTree {
    pub files: HashMap<u32, AsmFile>,
    pub models: HashMap<u32, AsmModel>,
    pub root_model: Option<u32>,
}

/// A flattened component instance (depth-first order).
#[derive(Debug, Clone)]
pub struct Component {
    /// Display name (instance name, e.g. `bolt`), shown in the tree.
    pub name: String,
    /// Document path as stored (may be an absolute path from the author's machine).
    pub path: String,
    pub is_assembly: bool,
    pub depth: usize,
    pub parent: Option<usize>,
    /// World transform (column-vector convention, metres).
    pub world: DMat4,
    pub suppressed: bool,
    pub hidden: bool,
}

fn parse_matrix(s: &str) -> Option<DMat4> {
    let v: Vec<f64> = s
        .split_whitespace()
        .filter_map(|x| x.parse().ok())
        .collect();
    if v.len() < 16 {
        return None;
    }
    let mut a = [0.0; 16];
    a.copy_from_slice(&v[..16]);
    Some(DMat4::from_cols_array(&a))
}

fn yes(s: Option<&str>) -> bool {
    matches!(s, Some(x) if x.eq_ignore_ascii_case("yes") || x == "1" || x.eq_ignore_ascii_case("true"))
}

/// Decode XML bytes (UTF-8 or UTF-16 with BOM).
fn decode_text(b: &[u8]) -> String {
    if b.starts_with(&[0xff, 0xfe]) || b.starts_with(&[0xfe, 0xff]) {
        let le = b[0] == 0xff;
        let u: Vec<u16> = b[2..]
            .chunks_exact(2)
            .map(|c| {
                if le {
                    u16::from_le_bytes([c[0], c[1]])
                } else {
                    u16::from_be_bytes([c[0], c[1]])
                }
            })
            .collect();
        String::from_utf16_lossy(&u)
    } else {
        let b = b.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(b);
        String::from_utf8_lossy(b).into_owned()
    }
}

pub fn parse(xml: &[u8]) -> Result<AsmTree, String> {
    let text = decode_text(xml);
    let text = text.trim_end_matches('\0');
    let doc = roxmltree::Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    )
    .map_err(|e| format!("COMPINSTANCETREE: {e}"))?;
    let mut t = AsmTree::default();
    let mut root_from_recent = false;
    let id = |n: &roxmltree::Node| n.attribute("id").and_then(|x| x.parse::<u32>().ok());
    for n in doc.descendants() {
        match n.tag_name().name() {
            "swFile" => {
                if let Some(i) = id(&n) {
                    t.files.insert(
                        i,
                        AsmFile {
                            path: n.attribute("swPath").unwrap_or("").to_string(),
                            is_assembly: n
                                .attribute("swDocType")
                                .is_some_and(|x| x.eq_ignore_ascii_case("ASSEMBLY")),
                        },
                    );
                }
            }
            "swModel" => {
                if let Some(i) = id(&n) {
                    let refs = n
                        .children()
                        .filter(|c| c.tag_name().name() == "swReference")
                        .filter_map(|c| {
                            Some(AsmReference {
                                name: c.attribute("swName").unwrap_or("?").to_string(),
                                model: c.attribute("swModelRef")?.parse().ok()?,
                                transform: c
                                    .attribute("swTransform")
                                    .and_then(parse_matrix)
                                    .unwrap_or(DMat4::IDENTITY),
                                suppressed: yes(c.attribute("swSuppressed")),
                                hidden: yes(c.attribute("swHidden")),
                            })
                        })
                        .collect();
                    t.models.insert(
                        i,
                        AsmModel {
                            name: n.attribute("swName").unwrap_or("").to_string(),
                            config: n.attribute("swConfigurationName").unwrap_or("").to_string(),
                            file: n.attribute("swFileRef").and_then(|x| x.parse().ok()),
                            refs,
                        },
                    );
                }
            }
            "swConfiguration" => {
                let recent = yes(n.attribute("swMostRecentConfiguration"));
                if let Some(m) = n
                    .attribute("swModelRef")
                    .and_then(|x| x.parse::<u32>().ok())
                {
                    if (recent && !root_from_recent) || t.root_model.is_none() {
                        t.root_model = Some(m);
                        root_from_recent |= recent;
                    }
                }
            }
            _ => {}
        }
    }
    if t.root_model.is_none() {
        // Fallback: the model that is not referenced by anyone and has references.
        let referenced: std::collections::HashSet<u32> = t
            .models
            .values()
            .flat_map(|m| m.refs.iter().map(|r| r.model))
            .collect();
        t.root_model = t
            .models
            .iter()
            .filter(|(k, m)| !referenced.contains(k) && !m.refs.is_empty())
            .map(|(k, _)| *k)
            .min();
    }
    Ok(t)
}

impl AsmTree {
    /// Flatten into a depth-first list of component instances with world transforms.
    pub fn flatten(&self) -> Vec<Component> {
        let mut out = Vec::new();
        if let Some(r) = self.root_model {
            self.walk(
                r,
                DMat4::IDENTITY,
                0,
                None,
                false,
                false,
                &mut out,
                &mut Vec::new(),
            );
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn walk(
        &self,
        model: u32,
        world: DMat4,
        depth: usize,
        parent: Option<usize>,
        sup: bool,
        hid: bool,
        out: &mut Vec<Component>,
        stack: &mut Vec<u32>,
    ) {
        if stack.contains(&model) || depth > 64 || out.len() > 200_000 {
            return;
        }
        let Some(m) = self.models.get(&model) else {
            return;
        };
        stack.push(model);
        for r in &m.refs {
            let w = world * r.transform;
            let child = self.models.get(&r.model);
            let file = child.and_then(|c| c.file).and_then(|f| self.files.get(&f));
            let idx = out.len();
            out.push(Component {
                name: r.name.clone(),
                path: file.map(|f| f.path.clone()).unwrap_or_default(),
                is_assembly: file.is_some_and(|f| f.is_assembly),
                depth,
                parent,
                world: w,
                suppressed: sup || r.suppressed,
                hidden: hid || r.hidden,
            });
            self.walk(
                r.model,
                w,
                depth + 1,
                Some(idx),
                sup || r.suppressed,
                hid || r.hidden,
                out,
                stack,
            );
        }
        stack.pop();
    }
}
