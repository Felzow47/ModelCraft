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
    Command { id: "edit.hide", label: "Masquer la sélection", icon: "eye-off", shortcut: ks(N, Key::H), group: "Édition" },
    Command { id: "edit.isolate", label: "Isoler la sélection", icon: "focus", shortcut: ks(N, Key::I), group: "Édition" },
    Command { id: "edit.show_all", label: "Afficher tout", icon: "eye", shortcut: ks(S, Key::H), group: "Édition" },
    Command { id: "edit.deselect", label: "Tout désélectionner", icon: "square-dashed", shortcut: ks(N, Key::Escape), group: "Édition" },
    Command { id: "edit.copy_dims", label: "Copier les dimensions", icon: "copy", shortcut: ks(CS, Key::C), group: "Édition" },
    Command { id: "edit.settings", label: "Paramètres…", icon: "settings", shortcut: ks(C, Key::Comma), group: "Édition" },
    // Affichage
    Command { id: "view.fit", label: "Ajuster à l'écran", icon: "maximize", shortcut: ks(N, Key::F), group: "Affichage" },
    Command { id: "view.iso", label: "Vue isométrique", icon: "box", shortcut: ks(N, Key::Num0), group: "Affichage" },
    Command { id: "view.front", label: "Vue de face", icon: "square-dashed", shortcut: ks(N, Key::Num1), group: "Affichage" },
    Command { id: "view.top", label: "Vue de dessus", icon: "square-dashed", shortcut: ks(N, Key::Num2), group: "Affichage" },
    Command { id: "view.right", label: "Vue de droite", icon: "square-dashed", shortcut: ks(N, Key::Num3), group: "Affichage" },
    Command { id: "view.back", label: "Vue arrière", icon: "square-dashed", shortcut: ks(N, Key::Num4), group: "Affichage" },
    Command { id: "view.bottom", label: "Vue de dessous", icon: "square-dashed", shortcut: ks(N, Key::Num5), group: "Affichage" },
    Command { id: "view.left", label: "Vue de gauche", icon: "square-dashed", shortcut: ks(N, Key::Num6), group: "Affichage" },
    Command { id: "view.ortho", label: "Projection orthographique", icon: "cuboid", shortcut: ks(N, Key::O), group: "Affichage" },
    Command { id: "view.edges", label: "Arêtes", icon: "spline-pointer", shortcut: ks(N, Key::E), group: "Affichage" },
    Command { id: "view.bbox", label: "Boîte englobante", icon: "scan", shortcut: ks(N, Key::B), group: "Affichage" },
    Command { id: "view.section", label: "Plan de coupe", icon: "scissors", shortcut: ks(N, Key::X), group: "Affichage" },
    Command { id: "view.grid", label: "Grille", icon: "grid-3x3", shortcut: ks(N, Key::G), group: "Affichage" },
    Command { id: "view.axes", label: "Repère XYZ", icon: "axis-3d", shortcut: None, group: "Affichage" },
    Command { id: "view.viewcube", label: "Cube de vue", icon: "box", shortcut: None, group: "Affichage" },
    Command { id: "view.tree", label: "Arbre de création", icon: "list-tree", shortcut: ks(N, Key::T), group: "Affichage" },
    Command { id: "view.info", label: "Panneau d'informations", icon: "panel-right", shortcut: ks(N, Key::P), group: "Affichage" },
    Command { id: "view.theme", label: "Basculer thème clair / sombre", icon: "moon", shortcut: None, group: "Affichage" },
    Command { id: "view.fullscreen", label: "Plein écran", icon: "maximize", shortcut: ks(N, Key::F11), group: "Affichage" },
    // Outils
    Command { id: "tool.select", label: "Sélection", icon: "mouse-pointer-2", shortcut: ks(N, Key::S), group: "Outils" },
    Command { id: "tool.distance", label: "Mesurer une distance", icon: "ruler", shortcut: ks(N, Key::D), group: "Outils" },
    Command { id: "tool.edge", label: "Mesurer une arête", icon: "spline-pointer", shortcut: ks(N, Key::L), group: "Outils" },
    Command { id: "tool.diameter", label: "Mesurer un diamètre", icon: "diameter", shortcut: ks(N, Key::R), group: "Outils" },
    Command { id: "tool.angle", label: "Mesurer un angle", icon: "drafting-compass", shortcut: ks(N, Key::A), group: "Outils" },
    Command { id: "tool.clear", label: "Effacer les mesures", icon: "eraser", shortcut: ks(N, Key::Delete), group: "Outils" },
    Command { id: "asm.move", label: "Déplacer des composants", icon: "move-3d", shortcut: ks(N, Key::M), group: "Outils" },
    Command { id: "asm.rotate", label: "Poignées de rotation", icon: "rotate-3d", shortcut: None, group: "Outils" },
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

pub fn shortcut_text(ctx: &egui::Context, id: &str) -> String {
    get(id).shortcut.map(|s| ctx.format_shortcut(&s)).unwrap_or_default()
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
        let mut sc: Vec<_> = COMMANDS.iter().filter_map(|c| c.shortcut).map(|s| format!("{:?}", s)).collect();
        sc.sort();
        let n = sc.len();
        sc.dedup();
        assert_eq!(n, sc.len());
    }
}
