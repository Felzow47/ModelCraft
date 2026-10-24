# ModelCraft — mentions et attributions

ModelCraft est un projet **indépendant**. Il n'est ni un produit officiel de la suite ArtCraft
ni affilié à l'ArtCraft Team, à Dassault Systèmes (SolidWorks, eDrawings, 3DXML) ou à Autodesk (3DS).
Les noms ArtCraft, SolidWorks, eDrawings, Parasolid, 3ds Max… appartiennent à leurs propriétaires.
Aucun logo ArtCraft n'est utilisé : le logo ModelCraft (`assets/logo/`) est une création originale.

**Aucun code d'eDrawings ni de SolidWorks** n'a été utilisé, désassemblé ou recopié.

## Code repris

| Élément | Origine | Licence |
|---|---|---|
| Disposition de l'interface (barre d'onglets, barre de modes, accueil, palette Ctrl+K, jetons de design, widgets) — `crates/swcraft/src/{theme,widgets,chrome,home,palette}.rs` | [PdfCraft](https://github.com/storytold/pdfcraft), © 2026 ArtCraft Team et contributeurs PdfCraft | MIT OR Apache-2.0 — `licenses/LICENSE-PdfCraft-MIT.txt` |

## Ressources tierces intégrées

| Élément | Licence | Fichier |
|---|---|---|
| Icônes [Lucide](https://lucide.dev) (`crates/swcraft/assets/icons/*.svg`) | ISC | `licenses/LICENSE-lucide.txt` |
| Police Inter (`Inter-*.ttf`) | SIL OFL 1.1 | `licenses/OFL-Inter.txt` |
| Police JetBrains Mono | SIL OFL 1.1 | `licenses/OFL-JetBrainsMono.txt` |
| [Open CASCADE Technology](https://dev.opencascade.org) 7.8.1 — lecture STEP / IGES (fonctionnalité `occt`) | LGPL-2.1 avec exception OCCT | voir ci-dessous |

### Open CASCADE (LGPL-2.1)

Les exécutables Windows de `dist/` sont liés **statiquement** à OCCT 7.8.1 non modifié.
Conformément à la LGPL, vous pouvez relier ModelCraft à une autre version d'OCCT : tout le
code source de ModelCraft est fourni ici et la procédure de compilation (section
« Compiler depuis les sources » du README, variables `OCCT_INC` / `OCCT_LIB`) permet de
reconstruire l'exécutable avec votre propre build d'OCCT. Source OCCT :
https://github.com/Open-Cascade-SAS/OCCT/releases/tag/V7_8_1

Les autres dépendances Rust (egui, eframe, wgpu, glam, zip, roxmltree, image, rfd, winreg…)
sont sous licences MIT / Apache-2.0 / zlib ; la liste complète est dans `Cargo.lock`.
