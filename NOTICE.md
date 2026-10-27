# ModelCraft — mentions et attributions

ModelCraft est un projet **indépendant**. Il n'est ni un produit officiel de la suite ArtCraft
ni affilié à l'ArtCraft Team, à Dassault Systèmes (SolidWorks, eDrawings, 3DXML) ou à Autodesk (3DS, Revit).
Les noms ArtCraft, SolidWorks, eDrawings, Parasolid, 3ds Max… appartiennent à leurs propriétaires.
Aucun logo ArtCraft n'est utilisé : le logo ModelCraft (`assets/logo/`) est une création originale.

**Aucun code d'eDrawings, de SolidWorks ni de Revit** n'a été utilisé, désassemblé ou recopié.
Le lecteur Revit (`crates/craft-formats/src/rvt.rs`) est une implémentation indépendante qui lit
le conteneur OLE avec la bibliothèque `cfb` (MIT).

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
| [Open Asset Import Library (assimp)](https://github.com/assimp/assimp) 6.0.2 — FBX, glTF, COLLADA, 3MF, PLY, AMF, X3D, DXF, USD… (fonctionnalité `assimp`, lien statique, importeur VRML désactivé car il embarque du code GPL) | BSD-3-Clause | `licenses/LICENSE-assimp-BSD3.txt` |
| ↳ composants inclus dans assimp : tinyusdz (Apache-2.0), pugixml, rapidjson, Open3DGC (MIT), poly2tri (BSD-3), clipper (Boost), openddl-parser (MIT), utf8cpp (BSL-1.0), earcut.hpp (ISC), stb (domaine public / MIT), zlib + minizip (zlib) | voir chaque projet | `licenses/LICENSE-tinyusdz-Apache2.txt`, `licenses/LICENSE-assimp-BSD3.txt` |
| [web-ifc](https://github.com/ThatOpen/engine_web-ifc) (That Open Company), commit `6b33ddf`, **non modifié** — lecture IFC 2x3 / 4 / 4x3 (fonctionnalité `ifc`, lien statique) | MPL-2.0 | `licenses/LICENSE-web-ifc-MPL2.txt` |
| ↳ dépendances de web-ifc : CDT (MPL-2.0), glm (MIT), tinynurbs (BSD-3), earcut.hpp (ISC), fast_float (MIT), spdlog + fmt (MIT), stduuid (MIT), unordered_dense (MIT) | voir chaque projet | `licenses/LICENSE-{CDT-MPL2,glm-MIT,tinynurbs-BSD3,earcut-ISC,fast_float-MIT,spdlog-MIT,stduuid-MIT,unordered_dense-MIT}.txt` |

### Open CASCADE (LGPL-2.1)

Les exécutables Windows de `dist/` sont liés **statiquement** à OCCT 7.8.1 non modifié.
Conformément à la LGPL, vous pouvez relier ModelCraft à une autre version d'OCCT : tout le
code source de ModelCraft est fourni ici et la procédure de compilation (section
« Compiler depuis les sources » du README, variables `OCCT_INC` / `OCCT_LIB`) permet de
reconstruire l'exécutable avec votre propre build d'OCCT. Source OCCT :
https://github.com/Open-Cascade-SAS/OCCT/releases/tag/V7_8_1

### web-ifc et CDT (MPL-2.0)

Les fichiers sous MPL-2.0 (web-ifc, CDT) sont utilisés **sans modification** ; leur source
est disponible aux adresses https://github.com/ThatOpen/engine_web-ifc (commit
`6b33ddf26c74d4881febaabf7a5bcb5259eccd96`) et https://github.com/artem-ogre/CDT. Seule
compilation particulière : `-Dmapbox=wi_mapbox` (renomme l'espace de noms d'earcut pour
éviter un conflit avec la copie d'earcut incluse dans assimp). Le reste de ModelCraft n'est
pas soumis à la MPL (couverture fichier par fichier).

### Formats non pris en charge pour raison de licence

- **DWG** : la seule bibliothèque libre complète (LibreDWG) est sous **GPL-3** ; l'intégrer
  imposerait la GPL à tout l'exécutable. Non incluse.
- **SketchUp `.skp`** : seul le SDK propriétaire de Trimble sait le lire. Non inclus.
- **Rhino `.3dm`** : openNURBS est sous licence permissive, mais il fournit des NURBS sans
  maillage de rendu garanti ; non intégré dans cette version.

Les autres dépendances Rust (egui, eframe, wgpu, glam, zip, roxmltree, image, rfd, winreg, cfb, memmap2…)
sont sous licences MIT / Apache-2.0 / zlib ; la liste complète est dans `Cargo.lock`.

### Installateur

`dist/ModelCraft-Setup-*.exe` est produit avec **Inno Setup** (Jordan Russell / Martijn Laan,
licence Inno Setup, gratuite y compris pour un usage commercial). Le script `installer/modelcraft.iss`
est généré par `installer/gen_iss.py` (code ModelCraft). Les solutions open source
*solidcraft* et *BIMCraft* ont été étudiées uniquement comme sources d'idées (voir
`docs/REFERENCES.md`) : aucune ligne de leur code n'est reprise.
