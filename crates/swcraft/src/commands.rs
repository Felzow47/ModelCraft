//! Every action of the app, shared by the menus, the keyboard and the command palette (Ctrl+K).

use egui::{Key, KeyboardShortcut, Modifiers};

pub struct Command {
    pub id: &'static str,
    /// French label (translated with `tr`).
    pub label: &'static str,
    pub icon: &'static str,
    pub shortcut: Option<KeyboardShortcut>,
    /// Menu the command belongs to (palette group).
    pub group: &'static str,
}

const fn ks(m: Modifiers, k: Key) -> Option<KeyboardShortcut> {
    Some(KeyboardShortcut::new(m, k))
}
const N: Modifiers = Modifiers::NONE;
const C: Modifiers = Modifiers::COMMAND;
const S: Modifiers = Modifiers::SHIFT;
const CS: Modifiers = Modifiers { alt: false, ctrl: false, shift: true, mac_cmd: false, command: true };

/// Extra keys for some commands (Blender habits, keyboard layouts that send "," for Numpad .).
pub static EXTRA_KEYS: &[(KeyboardShortcut, &str)] = &[
    (KeyboardShortcut::new(CS, Key::Z), "edit.redo"),
    (KeyboardShortcut::new(N, Key::Comma), "view.frame_selected"),
    (KeyboardShortcut::new(N, Key::Home), "view.fit"),
    (KeyboardShortcut::new(N, Key::Num5), "view.ortho"),
    (KeyboardShortcut::new(N, Key::M), "asm.move"),
    (KeyboardShortcut::new(Modifiers::ALT, Key::H), "edit.show_all"),
    (KeyboardShortcut::new(N, Key::Slash), "edit.isolate"),
    (KeyboardShortcut::new(N, Key::N), "view.info"),
];

pub static COMMANDS: &[Command] = &[
    // Fichier
    Command { id: "file.open", label: "Ouvrir…", icon: "folder-open", shortcut: ks(C, Key::O), group: "Fichier" },
    Command { id: "file.home", label: "Accueil", icon: "house", shortcut: None, group: "Fichier" },
    Command { id: "file.close", label: "Fermer l'onglet", icon: "x", shortcut: ks(C, Key::W), group: "Fichier" },
    Command { id: "file.reload", label: "Recharger le fichier", icon: "refresh-cw", shortcut: ks(N, Key::F5), group: "Fichier" },
    Command { id: "file.reveal", label: "Afficher dans l'explorateur", icon: "external-link", shortcut: None, group: "Fichier" },
    Command { id: "file.copy_path", label: "Copier le chemin du fichier", icon: "copy", shortcut: None, group: "Fichier" },
    Command { id: "file.screenshot", label: "Exporter une capture PNG…", icon: "camera", shortcut: ks(CS, Key::S), group: "Fichier" },
    Command { id: "file.quit", label: "Quitter", icon: "x", shortcut: ks(C, Key::Q), group: "Fichier" },
    // Édition
    Command { id: "edit.undo", label: "Annuler", icon: "undo-2", shortcut: ks(C, Key::Z), group: "Édition" },
    Command { id: "edit.redo", label: "Rétablir", icon: "redo-2", shortcut: ks(C, Key::Y), group: "Édition" },
    Command { id: "edit.hide", label: "Masquer la sélection", icon: "eye-off", shortcut: ks(N, Key::H), group: "Édition" },
    Command { id: "edit.isolate", label: "Isoler la sélection", icon: "focus", shortcut: ks(N, Key::I), group: "Édition" },
    Command { id: "edit.show_all", label: "Afficher tout", icon: "eye", shortcut: ks(S, Key::H), group: "Édition" },
    Command { id: "edit.deselect", label: "Tout désélectionner", icon: "square-dashed", shortcut: ks(N, Key::Escape), group: "Édition" },
    Command { id: "edit.copy_dims", label: "Copier les dimensions", icon: "copy", shortcut: ks(CS, Key::C), group: "Édition" },
    Command { id: "edit.settings", label: "Paramètres…", icon: "settings", shortcut: ks(C, Key::Comma), group: "Édition" },
    // Affichage
    Command { id: "view.fit", label: "Ajuster à l'écran", icon: "maximize", shortcut: ks(N, Key::F), group: "Affichage" },
    Command { id: "view.frame_selected", label: "Cadrer la sélection", icon: "scan-search", shortcut: ks(N, Key::Period), group: "Affichage" },
    Command { id: "view.iso", label: "Vue isométrique", icon: "box", shortcut: ks(N, Key::Num0), group: "Affichage" },
    Command { id: "view.front", label: "Vue de face", icon: "square-dashed", shortcut: ks(N, Key::Num1), group: "Affichage" },
    Command { id: "view.top", label: "Vue de dessus", icon: "square-dashed", shortcut: ks(N, Key::Num7), group: "Affichage" },
    Command { id: "view.right", label: "Vue de droite", icon: "square-dashed", shortcut: ks(N, Key::Num3), group: "Affichage" },
    Command { id: "view.back", label: "Vue arrière", icon: "square-dashed", shortcut: ks(C, Key::Num1), group: "Affichage" },
    Command { id: "view.bottom", label: "Vue de dessous", icon: "square-dashed", shortcut: ks(C, Key::Num7), group: "Affichage" },
    Command { id: "view.left", label: "Vue de gauche", icon: "square-dashed", shortcut: ks(C, Key::Num3), group: "Affichage" },
    Command { id: "view.flip", label: "Vue opposée", icon: "refresh-cw", shortcut: ks(N, Key::Num9), group: "Affichage" },
    Command { id: "view.orbit_left", label: "Tourner la vue à gauche (15°)", icon: "rotate-ccw", shortcut: ks(N, Key::Num4), group: "Affichage" },
    Command { id: "view.orbit_right", label: "Tourner la vue à droite (15°)", icon: "rotate-cw", shortcut: ks(N, Key::Num6), group: "Affichage" },
    Command { id: "view.orbit_up", label: "Tourner la vue vers le haut (15°)", icon: "rotate-ccw", shortcut: ks(N, Key::Num8), group: "Affichage" },
    Command { id: "view.orbit_down", label: "Tourner la vue vers le bas (15°)", icon: "rotate-cw", shortcut: ks(N, Key::Num2), group: "Affichage" },
    Command { id: "view.ortho", label: "Projection orthographique", icon: "cuboid", shortcut: ks(N, Key::O), group: "Affichage" },
    Command { id: "view.edges", label: "Arêtes", icon: "pen-line", shortcut: ks(N, Key::E), group: "Affichage" },
    Command { id: "view.bbox", label: "Boîte englobante", icon: "scan", shortcut: ks(N, Key::B), group: "Affichage" },
    Command { id: "view.section", label: "Plan de coupe", icon: "scissors", shortcut: ks(N, Key::X), group: "Affichage" },
    Command { id: "view.grid", label: "Grille", icon: "grid-3x3", shortcut: ks(S, Key::G), group: "Affichage" },
    Command { id: "view.axes", label: "Repère XYZ", icon: "axis-3d", shortcut: None, group: "Affichage" },
    Command { id: "view.free_orbit_roll", label: "Rotation libre après pivot du cube", icon: "rotate-3d", shortcut: None, group: "Affichage" },
    Command { id: "asm.face_detach", label: "Déplacer une face seule", icon: "square-split-horizontal", shortcut: None, group: "Outils" },
    Command { id: "view.viewcube", label: "Cube de vue", icon: "box", shortcut: None, group: "Affichage" },
    Command { id: "view.explode", label: "Vue éclatée…", icon: "boxes", shortcut: ks(S, Key::V), group: "Affichage" },
    Command { id: "view.tree", label: "Arbre de création", icon: "list-tree", shortcut: ks(N, Key::T), group: "Affichage" },
    Command { id: "view.info", label: "Panneau d'informations", icon: "panel-right", shortcut: ks(N, Key::P), group: "Affichage" },
    Command { id: "view.theme", label: "Basculer thème clair / sombre", icon: "moon", shortcut: None, group: "Affichage" },
    Command { id: "view.fullscreen", label: "Plein écran", icon: "maximize", shortcut: ks(N, Key::F11), group: "Affichage" },
    // Outils
    Command { id: "tool.select", label: "Sélection", icon: "mouse-pointer-2", shortcut: ks(N, Key::S), group: "Outils" },
    Command { id: "tool.pan", label: "Panoramique (déplacer la vue)", icon: "hand", shortcut: ks(N, Key::Q), group: "Outils" },
    Command { id: "tool.distance", label: "Mesurer une distance", icon: "ruler", shortcut: ks(N, Key::D), group: "Outils" },
    Command { id: "tool.edge", label: "Mesurer une arête", icon: "spline-pointer", shortcut: ks(N, Key::L), group: "Outils" },
    Command { id: "tool.diameter", label: "Mesurer un diamètre", icon: "diameter", shortcut: ks(S, Key::D), group: "Outils" },
    Command { id: "tool.angle", label: "Mesurer un angle", icon: "drafting-compass", shortcut: ks(N, Key::A), group: "Outils" },
    Command { id: "tool.clear", label: "Effacer les mesures", icon: "eraser", shortcut: ks(N, Key::Delete), group: "Outils" },
    Command { id: "asm.move", label: "Déplacer des composants", icon: "move-3d", shortcut: ks(N, Key::G), group: "Outils" },
    Command { id: "asm.rotate", label: "Tourner des composants", icon: "rotate-3d", shortcut: ks(N, Key::R), group: "Outils" },
    Command { id: "pivot.bbox", label: "Pivot : centre de la boîte englobante", icon: "scan", shortcut: None, group: "Outils" },
    Command { id: "pivot.object", label: "Pivot : centre de l'objet", icon: "box", shortcut: None, group: "Outils" },
    Command { id: "pivot.cursor", label: "Pivot : curseur 3D", icon: "crosshair", shortcut: None, group: "Outils" },
    Command { id: "pivot.world", label: "Pivot : origine du monde", icon: "globe", shortcut: None, group: "Outils" },
    Command { id: "pivot.cursor_to_sel", label: "Curseur 3D sur la sélection", icon: "locate-fixed", shortcut: None, group: "Outils" },
    Command { id: "asm.reset_sel", label: "Réinitialiser la position", icon: "rotate-ccw", shortcut: None, group: "Outils" },
    Command { id: "asm.reset_all", label: "Réinitialiser toutes les positions", icon: "rotate-ccw", shortcut: ks(S, Key::M), group: "Outils" },
    Command { id: "asm.explode", label: "Vue éclatée", icon: "boxes", shortcut: ks(N, Key::V), group: "Outils" },
    Command { id: "asm.explode_lines", label: "Lignes d'éclatement", icon: "spline-pointer", shortcut: None, group: "Outils" },
    Command { id: "asm.transparent", label: "Rendre la sélection transparente", icon: "layers", shortcut: ks(N, Key::J), group: "Outils" },
    Command { id: "asm.others", label: "Transparence des autres", icon: "layers", shortcut: ks(S, Key::J), group: "Outils" },
    Command { id: "asm.xray", label: "Transparence globale", icon: "eye", shortcut: None, group: "Outils" },
    Command { id: "tool.assoc", label: "Associer les fichiers à ModelCraft", icon: "link", shortcut: None, group: "Outils" },
    Command { id: "tool.settings_folder", label: "Ouvrir le dossier des paramètres", icon: "folder", shortcut: None, group: "Outils" },
    // Aide
    Command { id: "help.palette", label: "Palette de commandes", icon: "command", shortcut: ks(C, Key::K), group: "Aide" },
    Command { id: "help.shortcuts", label: "Raccourcis clavier", icon: "keyboard", shortcut: ks(N, Key::F1), group: "Aide" },
    Command { id: "help.formats", label: "Formats pris en charge", icon: "file-box", shortcut: None, group: "Aide" },
    Command { id: "help.about", label: "À propos de ModelCraft", icon: "info", shortcut: None, group: "Aide" },
];

pub fn get(id: &str) -> &'static Command {
    COMMANDS.iter().find(|c| c.id == id).unwrap_or_else(|| panic!("unknown command {id}"))
}

/// "Raccourcis Blender" setting (on by default), mirrored here so tooltips show the active keys.
pub static BLENDER_KEYS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Previous ModelCraft keys, used when the Blender keymap is turned off.
static CLASSIC_KEYS: &[(KeyboardShortcut, &str)] = &[
    (KeyboardShortcut::new(N, Key::Num0), "view.iso"),
    (KeyboardShortcut::new(N, Key::Num1), "view.front"),
    (KeyboardShortcut::new(N, Key::Num2), "view.top"),
    (KeyboardShortcut::new(N, Key::Num3), "view.right"),
    (KeyboardShortcut::new(N, Key::Num4), "view.back"),
    (KeyboardShortcut::new(N, Key::Num5), "view.bottom"),
    (KeyboardShortcut::new(N, Key::Num6), "view.left"),
    (KeyboardShortcut::new(N, Key::G), "view.grid"),
    (KeyboardShortcut::new(N, Key::R), "tool.diameter"),
];

pub fn shortcut_text(ctx: &egui::Context, id: &str) -> String {
    let all = bindings();
    // the command's own key first, then any other binding
    let own = get(id).shortcut.filter(|s| all.iter().any(|(b, i)| b == s && *i == id));
    own.or_else(|| all.iter().find(|(_, i)| *i == id).map(|(s, _)| *s)).map(|s| key_text(ctx, &s)).unwrap_or_default()
}

/// egui spells symbol keys by name ("Period"); show the symbol, and Home in French.
pub fn key_text(ctx: &egui::Context, s: &KeyboardShortcut) -> String {
    let t = ctx.format_shortcut(s);
    let sym = match s.logical_key {
        Key::Period => ".",
        Key::Comma => ",",
        Key::Slash => "/",
        Key::Minus => "-",
        Key::Home => crate::i18n::tr("Origine"),
        _ => return t,
    };
    match t.strip_suffix(s.logical_key.name()) {
        Some(m) => format!("{m}{sym}"),
        None => t,
    }
}

/// Every key binding (main + extra), most specific modifiers first: egui ignores extra Shift/Alt
/// when matching, so Shift+H must be tried before H.
pub fn bindings() -> Vec<(KeyboardShortcut, &'static str)> {
    let mut v: Vec<(KeyboardShortcut, &'static str)> = COMMANDS.iter().filter_map(|c| c.shortcut.map(|s| (s, c.id))).collect();
    v.extend(EXTRA_KEYS.iter().copied());
    if !BLENDER_KEYS.load(std::sync::atomic::Ordering::Relaxed) {
        // classic keys replace the Blender keys on the same keys (and Ctrl+numpad views)
        let classic_key = |k: Key| CLASSIC_KEYS.iter().any(|(c, _)| c.logical_key == k);
        v.retain(|(s, _)| !classic_key(s.logical_key));
        v.extend(CLASSIC_KEYS.iter().copied());
    }
    let weight = |m: Modifiers| m.shift as u8 + m.alt as u8 + (m.command || m.ctrl) as u8;
    v.sort_by_key(|(s, _)| std::cmp::Reverse(weight(s.modifiers)));
    v
}

/// Every key of a command, as text ("F or Home"), for the shortcuts dialog.
pub fn all_keys_text(ctx: &egui::Context, id: &str) -> String {
    let mut keys: Vec<String> = Vec::new();
    let main = shortcut_text(ctx, id);
    if !main.is_empty() {
        keys.push(main);
    }
    for (s, i) in bindings() {
        let k = key_text(ctx, &s);
        if i == id && !keys.contains(&k) {
            keys.push(k);
        }
    }
    keys.join(&format!("  {}  ", crate::i18n::tr("ou")))
}

/// Group labels in menu order.
pub const GROUPS: [&str; 5] = ["Fichier", "Édition", "Affichage", "Outils", "Aide"];

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ids_and_shortcuts_are_unique() {
        let mut ids: Vec<_> = COMMANDS.iter().map(|c| c.id).collect();
        ids.sort();
        let n = ids.len();
        ids.dedup();
        assert_eq!(n, ids.len());
        for (_, id) in EXTRA_KEYS {
            assert!(COMMANDS.iter().any(|c| c.id == *id), "{id}");
        }
        for blender in [true, false] {
            BLENDER_KEYS.store(blender, std::sync::atomic::Ordering::Relaxed);
            let mut sc: Vec<_> = bindings().iter().map(|(s, _)| format!("{:?}", s)).collect();
            sc.sort();
            let n = sc.len();
            sc.dedup();
            assert_eq!(n, sc.len(), "blender keys {blender}");
        }
        BLENDER_KEYS.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}
