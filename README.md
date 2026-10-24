# SW Craft — visionneuse SolidWorks rapide (V1)

Visionneuse native, légère et instantanée pour fichiers **SolidWorks `.SLDPRT` / `.SLDASM`**,
écrite en Rust (wgpu + egui). Pensée pour remplacer eDrawings pour la consultation :
démarrage immédiat, rotation fluide, interface sombre et épurée.

> SW Craft ne dépend **ni de SolidWorks ni d'eDrawings**. Il lit directement le maillage
> d'affichage (« DisplayLists ») que SolidWorks enregistre dans chaque fichier.
> Aucun code d'eDrawings n'a été utilisé.

## Contenu

| Élément | Rôle |
|---|---|
| `crates/sw-format` | Bibliothèque de lecture : conteneurs OLE (< 2015) et SW2015+ (ZIP déguisé), liste des flux, décodage des DisplayLists → maillages, vignettes (PNG/BMP), arbre d'assemblage + matrices de placement |
| `crates/swcraft` | L'application de visualisation (fenêtre, rendu 3D, arbre des composants) |
| `crates/swcraft-scan` | Outil en ligne de commande qui vérifie **en lecture seule** tout un dossier |
| `dist/` | Exécutables Windows x64 prêts à l'emploi : `swcraft.exe`, `swcraft-scan.exe` |

## Utilisation (Windows)

Copiez `swcraft.exe` où vous voulez (ex. `C:\Outils\SWCraft\`). Aucun installeur, aucune DLL.

Ouvrir un fichier :
- double-cliquez `swcraft.exe` puis **Ouvrir…** (ou `Ctrl+O`) ;
- ou **glissez-déposez** un `.SLDPRT` / `.SLDASM` dans la fenêtre ;
- ou en ligne de commande : `swcraft.exe "F:\chemin\piece.SLDPRT"`.

**Associer les fichiers SolidWorks à SW Craft** : clic droit sur un `.SLDPRT` →
*Ouvrir avec* → *Choisir une autre application* → *Plus d'applications* →
*Rechercher une autre application sur ce PC* → sélectionner `swcraft.exe` →
cocher *Toujours utiliser cette application*. Recommencer pour `.SLDASM`.

### Souris et clavier

| Action | Commande |
|---|---|
| Tourner | clic gauche + glisser |
| Déplacer | clic droit ou molette enfoncée + glisser (ou Maj + clic gauche) |
| Zoom (vers le curseur) | molette / pincement pavé tactile |
| Ajuster à la vue | double-clic, `F`, bouton **Ajuster** |
| Vues | `0` Iso · `1` Face · `2` Dessus · `3` Droite (boutons en haut) |
| Arêtes on/off | `E` |
| Perspective / orthographique | `O` |
| Arbre des composants | `T` |

Dans l'arbre (assemblages) : l'œil masque/affiche un composant (et ses enfants),
un clic le sélectionne et le met en surbrillance bleue, les flèches replient les
sous-assemblages. La barre d'état affiche le type, le format du fichier, le nombre
de faces/triangles et les temps de lecture.

Si un fichier ne contient **aucun maillage d'affichage**, SW Craft affiche la
**vignette** enregistrée par SolidWorks avec un message explicite.

Options de ligne de commande (utiles pour tests) :
```
swcraft.exe [fichier] [--view iso|front|top|right] [--size 1600x1000] [--no-edges] [--screenshot capture.png]
```

## Vérifier toute une archive : `swcraft-scan`

```
swcraft-scan.exe "F:\Dev\disneyAutoSorter\Sorted Archive IA" --csv C:\Temp\scan.csv --json C:\Temp\scan.json
```

- Parcourt le dossier récursivement et **n'écrit jamais dedans** (ouverture en lecture
  seule, liens/jonctions non suivis, fichiers verrous `~$…` ignorés).
- Le rapport CSV/JSON est **refusé** s'il devait être écrit dans le dossier scanné.
- Pour chaque fichier : `ok`, `partial` (assemblage dont des composants n'ont pas de
  maillage), `no_mesh` (vignette seulement), `error` (illisible) + faces, triangles,
  composants, vignette oui/non, temps en ms, message.
- Résumé final : comptes, pourcentages, temps médian / p95 / max, erreurs les plus fréquentes.
- Options : `--threads N` (défaut : nombre de cœurs, max 16), `--no-external`
  (ne pas chercher les pièces externes d'un assemblage), `--quiet`.
- Le CSV utilise `;` comme séparateur et un BOM UTF-8 (ouverture directe dans Excel FR).

## Compiler depuis les sources

Prérequis : [Rust](https://rustup.rs) ≥ 1.95.

Sous Windows (MSVC ou GNU) :
```
cargo build --release
target\release\swcraft.exe
```

Sous Linux :
```
sudo apt install libgtk-3-dev libxkbcommon-dev   # boîte de dialogue + clavier
cargo build --release
./target/release/swcraft fichier.SLDPRT
./target/release/swcraft-scan dossier/ --csv /tmp/scan.csv
```

Compilation croisée Linux → Windows (utilisée pour `dist/`) :
```
rustup target add x86_64-pc-windows-gnu
sudo apt install mingw-w64
cargo build --release --target x86_64-pc-windows-gnu -p swcraft -p swcraft-scan
```
(`.cargo/config.toml` configure l'éditeur de liens mingw et la CRT statique ; l'icône
et les infos de version sont intégrées via `windres`.)

Outil de diagnostic : `cargo run --release -p sw-format --example swinfo -- fichier.SLDASM [--streams] [--obj sortie.obj]`.

### Tests

Les fichiers SolidWorks ne sont **pas** dans le dépôt (`.gitignore`). Les tests
d'intégration lisent le dossier indiqué par `SW_SAMPLES` et sont ignorés s'il est absent :
```
SW_SAMPLES=/chemin/vers/echantillons cargo test -p sw-format
```

## Rendu

wgpu (DirectX 12 / Vulkan / OpenGL selon la machine), MSAA 4×, éclairage studio
(lumière principale + remplissage + ambiance hémisphérique + reflets), arêtes B-rep
superposées, fond dégradé. Si l'initialisation GPU échoue avec le moteur par défaut,
SW Craft se relance automatiquement en Vulkan puis en OpenGL. Forcer un moteur :
variable d'environnement `WGPU_BACKEND=dx12|vulkan|gl`.

## Limites connues (V1)

- Couleur unique gris-bleu : les couleurs/matériaux SolidWorks ne sont pas encore lus.
- Seul le maillage d'affichage enregistré est utilisé (pas de recalcul depuis le B-rep
  Parasolid) : un fichier enregistré sans données graphiques n'affiche que sa vignette.
- Assemblages : les pièces sont lues dans l'assemblage lui-même ; à défaut SW Craft
  cherche le fichier de la pièce (chemin d'origine, dossier de l'assemblage et dossier
  parent). Sinon le composant est marqué en orange dans l'arbre et le scan le signale `partial`.
- Pas encore de sélection par clic dans la vue 3D, de mesures ni de coupes.
- Validé sur des fichiers SolidWorks pré-2015 et 2015+ ; les versions les plus récentes
  (2020+) restent à confirmer sur l'archive complète via `swcraft-scan`.
