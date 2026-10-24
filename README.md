# ModelCraft — la visionneuse 3D ultime (V2)

<p align="center"><img src="screenshots/v2/logo.png" width="640" alt="Logo ModelCraft"></p>

Visionneuse native, légère et instantanée pour **SolidWorks (`.SLDPRT` / `.SLDASM`), STEP, IGES,
STL, OBJ, 3DS, 3DXML et eDrawings**, écrite en Rust (wgpu + egui). Un seul `.exe`, sans
installeur, sans licence, hors ligne.

Interface dans l'esprit de la suite ArtCraft (disposition reprise de PdfCraft, sous licence
MIT/Apache — voir `NOTICE.md`). ModelCraft est un projet **indépendant**, non affilié.

> ModelCraft ne dépend **ni de SolidWorks ni d'eDrawings** : il lit directement le maillage
> d'affichage enregistré dans les fichiers SolidWorks, et OpenCascade pour STEP/IGES.
> Aucun code d'eDrawings n'a été utilisé.

| Accueil | Assemblage + menu contextuel |
|---|---|
| ![Accueil](screenshots/v2/home_fr.png) | ![Arbre](screenshots/v2/ctx_tree.png) |
| **Paramètres** | **Palette de commandes (Ctrl+K)** |
| ![Paramètres](screenshots/v2/settings.png) | ![Palette](screenshots/v2/palette.png) |

## Formats

| Format | Extensions | Support | Remarques |
|---|---|---|---|
| SolidWorks | `.sldprt` `.sldasm` | ✅ complet | maillage d'affichage enregistré, arbre d'assemblage, couleurs ; vignette si aucun maillage |
| STEP | `.step` `.stp` | ✅ complet | via OpenCascade 7.8 : maillage des surfaces, arbre d'assemblage, couleurs |
| IGES | `.iges` `.igs` | ✅ complet | via OpenCascade |
| STL | `.stl` | ✅ | binaire et ASCII (unités : mm) |
| OBJ | `.obj` (+ `.mtl`) | ✅ | groupes/objets → composants, couleurs `Kd` |
| 3DS | `.3ds` | ✅ | objets et matériaux (couleur diffuse) |
| 3DXML | `.3dxml` | ✅ | représentations tessellées, assemblages |
| eDrawings | `.easm` `.eprt` | ⚠️ partiel | vignette intégrée seulement (géométrie HSF non décodée) |
| Parasolid | `.x_t` `.x_b` | ❌ | message clair : aucun lecteur libre — exporter en STEP |

## Utilisation (Windows)

Copiez `dist\modelcraft.exe` où vous voulez (ex. `C:\Outils\ModelCraft\`). Aucune DLL.

- **Ouvrir** : bouton **+ Ouvrir** / `Ctrl+O`, glisser-déposer (un dossier déposé est
  *épinglé* sur l'accueil), ou `modelcraft.exe "F:\chemin\piece.step"`. Plusieurs fichiers
  s'ouvrent dans des **onglets** (clic molette = fermer ; clic droit = fermer / recharger /
  afficher dans l'explorateur / copier le chemin).
- **Accueil** : formats pris en charge, **dossiers épinglés** (liste des modèles du dossier),
  **récents** avec chemin et taille (clic droit : ouvrir / afficher dans l'explorateur /
  copier le chemin / épingler le dossier / retirer).
- **Associer les extensions** : *Paramètres → Général → Associations de fichiers → Associer*
  (ou `modelcraft.exe --associate`). Ajoute ModelCraft à « Ouvrir avec » pour SLDPRT, SLDASM,
  STEP, IGES, STL, OBJ, 3DS, 3DXML, EASM — utilisateur courant (HKCU), sans droits admin.
- Chargement en arrière-plan avec progression (lecture → conversion → maillage) dans la barre d'état.

### Menus, palette, menus contextuels

- Barre de menus **Fichier / Édition / Affichage / Outils / Aide** avec raccourcis.
- **Ctrl+K** : palette de recherche de toutes les commandes et des fichiers récents.
- Clic droit **dans la vue 3D** : masquer / isoler / zoom sur la sélection, afficher tout,
  ajuster, vues standard, outils de mesure, projection, arêtes, boîte, coupe, grille, copier
  les dimensions, capture PNG.
- Clic droit **dans l'arbre** : masquer, isoler, afficher tout, zoom sur, copier le nom.

### Mesures (mode **Mesurer**)

| Outil | Résultat |
|---|---|
| Boîte englobante | L × l × h en surimpression (`B`), copiable (`Ctrl+Maj+C`) |
| Distance | entre deux points (accrochage sommets / milieux), avec ΔX ΔY ΔZ |
| Arête | longueur d'une arête |
| Diamètre | Ø d'un perçage / cylindre (ajustement sur la face) |
| Angle | entre deux faces |
| Face | surface de la face sélectionnée |
| Volume / surface | du modèle, dans le panneau Dimensions |
| Coupe | plan de coupe X/Y/Z réglable (`X`) |

Sélection de faces et de composants avec surbrillance ; le panneau de droite liste les mesures.

### Souris et clavier (navigation « SolidWorks », modifiable dans les paramètres)

| Action | Commande |
|---|---|
| Tourner | clic gauche + glisser |
| Déplacer | clic droit / molette enfoncée + glisser |
| Zoom (vers le curseur) | molette (inversible) |
| Ajuster | `F`, double-clic |
| Vues | `0` iso · `1` face · `2` dessus · `3` droite · `4` arrière · `5` dessous · `6` gauche, ou le **cube de vue** |
| Masquer / isoler / tout afficher | `H` / `I` / `Maj+H` |
| Arêtes · ortho · boîte · coupe · grille | `E` · `O` · `B` · `X` · `G` |
| Arbre · panneau d'infos | `T` · `P` |
| Paramètres · palette · plein écran | `Ctrl+,` · `Ctrl+K` · `F11` |

Styles de navigation disponibles : SolidWorks, Blender, AutoCAD/Inventor.

### Paramètres (`%APPDATA%\ModelCraft\settings.json`)

- **Général** : langue **Français / English** (détection automatique de la langue de Windows),
  thème clair / sombre / système, couleur d'accent, nombre de récents, associations.
- **Affichage** : fond dégradé ou uni, éclairage (studio, doux, contrasté…), couleur par
  défaut, arêtes (on/off, épaisseur, couleur), axes, grille, cube de vue.
- **Navigation** : style, zoom inversé, perspective / orthographique, champ de vision, vue par défaut.
- **Mesures** : unités (mm, cm, m, po), décimales.
- **Performances** : anticrénelage MSAA, qualité du maillage STEP/IGES.
- **Réinitialiser** remet toutes les valeurs par défaut.

Options de ligne de commande :
```
modelcraft.exe [fichiers…] [--lang fr|en] [--theme dark|light] [--view iso|front|top|right|back|bottom|left]
               [--size 1600x1000] [--no-edges] [--screenshot capture.png] [--ephemeral]
modelcraft.exe --associate
```

## Vérifier toute une archive : `swcraft-scan`

```
swcraft-scan.exe "F:\Dev\disneyAutoSorter\Sorted Archive IA" --csv C:\Temp\scan.csv --json C:\Temp\scan.json
```

- Parcourt le dossier récursivement et **n'écrit jamais dedans** (lecture seule, liens non
  suivis, fichiers `~$…` ignorés) ; un rapport ne peut pas être écrit dans le dossier scanné.
- Tous les formats de ModelCraft sont vérifiés (SolidWorks, STEP, IGES, STL, OBJ, 3DS, 3DXML,
  eDrawings, Parasolid) avec le même code que la visionneuse.
- Pour chaque fichier : format, `ok`, `partial` (composants sans maillage), `no_mesh`
  (vignette seulement), `unsupported` (ex. `.x_t`), `error` + faces, triangles, composants,
  temps, message. Résumé par format et temps médian / p95 / max.
- Options : `--threads N`, `--no-external`, `--quiet`. CSV séparé par `;` avec BOM (Excel FR).

## Compiler depuis les sources

Prérequis : [Rust](https://rustup.rs) ≥ 1.95.

Sous Windows (MSVC ou GNU) :
```
cargo build --release
target\release\modelcraft.exe
```

Sous Linux :
```
sudo apt install libgtk-3-dev libxkbcommon-dev   # boîte de dialogue + clavier
cargo build --release
./target/release/modelcraft fichier.SLDPRT
./target/release/swcraft-scan dossier/ --csv /tmp/scan.csv
```

**STEP / IGES** demandent la fonctionnalité `occt` et une compilation statique
d'[OpenCascade 7.8.1](https://github.com/Open-Cascade-SAS/OCCT) (`BUILD_LIBRARY_TYPE=Static`) :
```
OCCT_INC=/chemin/occt/include/opencascade OCCT_LIB=/chemin/occt/lib \
  cargo build --release -p swcraft -p swcraft-scan --features swcraft/occt,swcraft-scan/occt
```
Sans `occt`, ModelCraft fonctionne pour tous les autres formats et affiche un message pour STEP/IGES.

Compilation croisée Linux → Windows (utilisée pour `dist/`, OCCT compilé avec mingw « posix ») :
```
rustup target add x86_64-pc-windows-gnu
sudo apt install mingw-w64
CXX_x86_64_pc_windows_gnu=x86_64-w64-mingw32-g++-posix \
OCCT_INC=/chemin/occt/include/opencascade OCCT_LIB=/chemin/occt-mingw/win64/gcc/lib \
  cargo build --release --target x86_64-pc-windows-gnu -p swcraft -p swcraft-scan --features swcraft/occt,swcraft-scan/occt
```
(`.cargo/config.toml` configure l'éditeur de liens mingw et la CRT statique ; l'icône
et les infos de version sont intégrées via `windres`.)

Outil de diagnostic : `cargo run --release -p sw-format --example swinfo -- fichier.SLDASM [--streams] [--obj sortie.obj]`.

### Tests

Les fichiers de test (SolidWorks, STEP, maillages…) ne sont **pas** dans le dépôt (`.gitignore`). Les tests
d'intégration lisent le dossier indiqué par `SW_SAMPLES` et sont ignorés s'il est absent :
```
SW_SAMPLES=/chemin/vers/echantillons cargo test --release
```

## Rendu

wgpu (DirectX 12 / Vulkan / OpenGL selon la machine), MSAA réglable, éclairage studio,
arêtes B-rep superposées, fond dégradé, surbrillance de sélection, plan de coupe. Si
l'initialisation GPU échoue, ModelCraft se relance en Vulkan puis en OpenGL. Forcer un
moteur : `WGPU_BACKEND=dx12|vulkan|gl`.

## Limites connues (V2)

- **eDrawings `.easm`** : seule la vignette est affichée (géométrie HSF non décodée).
- **Parasolid `.x_t`** : non supporté (aucun lecteur libre) ; exporter en STEP.
- SolidWorks : seul le maillage d'affichage enregistré est utilisé (pas de recalcul B-rep) ;
  un fichier sans données graphiques n'affiche que sa vignette.
- Les messages d'erreur des lecteurs de fichiers restent en français dans l'interface anglaise.
- Mesures sur les maillages (STL/OBJ/3DS) : pas de diamètre exact (pas de faces analytiques).
- Pas d'export (STEP, STL…) ni d'annotations sauvegardées : visionneuse uniquement.

## Licence

MIT pour le code de ModelCraft. Code d'interface adapté de PdfCraft (MIT/Apache-2.0),
icônes Lucide (ISC), polices Inter et JetBrains Mono (OFL), OpenCascade (LGPL-2.1) :
détails dans [`NOTICE.md`](NOTICE.md).
