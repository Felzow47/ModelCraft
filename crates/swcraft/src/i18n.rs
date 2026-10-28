//! Interface language. French is the source language (strings are written in French in the
//! code); English comes from the table in `i18n_en.rs`. The OS language picks the default.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Lang {
    #[default]
    Fr,
    En,
}

/// The saved choice: follow the system, or a fixed language.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum LangPref {
    #[default]
    System,
    Fr,
    En,
}

impl LangPref {
    pub fn resolve(self) -> Lang {
        match self {
            LangPref::Fr => Lang::Fr,
            LangPref::En => Lang::En,
            LangPref::System => system_lang(),
        }
    }
}

/// French for any French locale (fr-FR, fr-CA, fr-BE…), English otherwise.
pub fn system_lang() -> Lang {
    lang_for_locale(&sys_locale::get_locale().unwrap_or_default())
}

pub fn lang_for_locale(loc: &str) -> Lang {
    if loc.to_ascii_lowercase().starts_with("fr") { Lang::Fr } else { Lang::En }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn set_current(l: Lang) {
    CURRENT.store(l as u8, Ordering::Relaxed);
}

pub fn current() -> Lang {
    if CURRENT.load(Ordering::Relaxed) == 1 { Lang::En } else { Lang::Fr }
}

fn table() -> &'static HashMap<&'static str, &'static str> {
    static T: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    T.get_or_init(|| crate::i18n_en::EN.iter().copied().collect())
}

/// Translate a French source string into the current language.
pub fn tr(fr: &'static str) -> &'static str {
    match current() {
        Lang::Fr => fr,
        Lang::En => table().get(fr).copied().unwrap_or(fr),
    }
}

/// Translate a runtime string (e.g. a loader warning) when the table knows it.
pub fn tr_str(fr: &str) -> String {
    match current() {
        Lang::Fr => fr.to_string(),
        Lang::En => table().get(fr).copied().unwrap_or(fr).to_string(),
    }
}

/// Translate then substitute `{key}` placeholders.
pub fn trf(fr: &'static str, args: &[(&str, &str)]) -> String {
    let mut s = tr(fr).to_string();
    for (k, v) in args {
        s = s.replace(&format!("{{{k}}}"), v);
    }
    s
}

#[macro_export]
macro_rules! tr {
    ($s:literal) => {
        $crate::i18n::tr($s)
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn french_locales_pick_french() {
        assert_eq!(lang_for_locale("fr-FR"), Lang::Fr);
        assert_eq!(lang_for_locale("fr_CA.UTF-8"), Lang::Fr);
        assert_eq!(lang_for_locale("en-US"), Lang::En);
        assert_eq!(lang_for_locale("de-DE"), Lang::En);
    }

    /// Every literal passed to `tr!` / `tr(` / `trf(` in the sources has an English entry.
    #[test]
    fn every_string_is_translated() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
        let mut missing = Vec::new();
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.extension().and_then(|x| x.to_str()) != Some("rs") || p.ends_with("i18n_en.rs") || p.ends_with("i18n.rs") {
                continue;
            }
            let src = std::fs::read_to_string(&p).unwrap();
            for pat in ["tr!(\"", "tr(\"", "trf(\""] {
                let mut rest = src.as_str();
                while let Some(i) = rest.find(pat) {
                    // `include_str!("`, `from_str("`… are not translations
                    let prev = rest[..i].chars().next_back();
                    rest = &rest[i + pat.len()..];
                    if prev.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                        continue;
                    }
                    // read a Rust string literal (handles \" escapes)
                    let mut lit = String::new();
                    let mut chars = rest.chars();
                    while let Some(c) = chars.next() {
                        match c {
                            '\\' => {
                                if let Some(n) = chars.next() {
                                    match n {
                                        'n' => lit.push('\n'),
                                        '"' => lit.push('"'),
                                        '\\' => lit.push('\\'),
                                        _ => {
                                            lit.push('\\');
                                            lit.push(n)
                                        }
                                    }
                                }
                            }
                            '"' => break,
                            _ => lit.push(c),
                        }
                    }
                    if !table().contains_key(lit.as_str()) {
                        missing.push(format!("{}: {lit}", p.file_name().unwrap().to_string_lossy()));
                    }
                }
            }
        }
        missing.sort();
        missing.dedup();
        assert!(missing.is_empty(), "{} chaînes sans traduction :\n{}", missing.len(), missing.join("\n"));
    }

    /// Strings translated through a variable (command labels, menus, cards, formats…).
    #[test]
    fn dynamic_strings_are_translated() {
        let mut all: Vec<&str> = Vec::new();
        all.extend(crate::commands::COMMANDS.iter().map(|c| c.label));
        all.extend(crate::commands::GROUPS);
        all.extend(crate::theme::Accent::ALL.iter().map(|a| a.label()));
        all.extend(crate::home::CARDS.iter().flat_map(|c| [c.title, c.blurb]));
        all.extend(crate::dialogs::CATS.iter().map(|c| c.2));
        all.extend(craft_formats::formats().iter().flat_map(|f| [f.name, f.note]));
        all.extend(crate::viewer::STAGES);
        let missing: Vec<&str> = all.into_iter().filter(|s| !table().contains_key(s)).collect();
        assert!(missing.is_empty(), "{} chaînes sans traduction :\n{}", missing.len(), missing.join("\n"));
    }
}
