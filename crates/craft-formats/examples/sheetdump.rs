//! Load a file and dump its sheets' strokes, fills and texts as plain lines (for previews).
//! Usage: sheetdump <file> <out.txt>
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let t = std::time::Instant::now();
    let s = match craft_formats::load_any(std::path::Path::new(&a[0]), &craft_formats::Options::default()) {
        Ok(s) => s,
        Err(e) => {
            println!("FAIL {e}");
            return;
        }
    };
    let mut o = String::new();
    for sh in &s.sheets {
        o += &format!("E {} {} {} {}\n", sh.extent[0], sh.extent[1], sh.extent[2], sh.extent[3]);
        for st in &sh.strokes {
            o += &format!("S {} {} {} {}\n", st.color[0], st.color[1], st.color[2], st.pts.iter().map(|p| format!("{} {}", p[0], p[1])).collect::<Vec<_>>().join(" "));
        }
        for f in &sh.fills {
            for tr in &f.tris {
                o += &format!("F {} {} {} {} {} {} {} {} {}\n", f.color[0], f.color[1], f.color[2], tr[0][0], tr[0][1], tr[1][0], tr[1][1], tr[2][0], tr[2][1]);
            }
        }
        for tx in &sh.texts {
            o += &format!("T {} {} {} {} {} {} {} {}\n", tx.pos[0], tx.pos[1], tx.dir[0], tx.dir[1], tx.height, tx.halign, tx.valign, tx.text.replace('\n', "\n"));
        }
    }
    std::fs::write(&a[1], o).unwrap();
    println!("ok {:.2}s sheets {} strokes {} fills {} texts {}", t.elapsed().as_secs_f64(), s.sheets.len(), s.sheets.iter().map(|x| x.strokes.len()).sum::<usize>(), s.sheets.iter().map(|x| x.fills.len()).sum::<usize>(), s.sheets.iter().map(|x| x.texts.len()).sum::<usize>());
}
