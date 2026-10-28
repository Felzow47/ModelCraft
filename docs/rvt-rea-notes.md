# Revit (.rvt/.rfa) - notes de rétro-ingénierie, phase 1 (salle blanche)

Date : 2026-10-28. But : lire la 3D des `.rvt` dans ModelCraft sans Revit à l'exécution.
Règles suivies : aucune action sur la licence / le login / l'activation Autodesk, Revit jamais
lancé par nous, rien modifié dans l'installation. Seulement des **observations d'octets** sur des
fichiers d'exemple fournis par Autodesk avec l'installation, et les **noms exportés** (table
d'export PE) d'une DLL. Pas de désassemblage ni de décompilation utilisés dans ces notes.
Aucun fichier Autodesk ni modèle client (NDA) n'est versionné ici.

## Installation observée (lecture seule)

- Revit 2025 build 25.0.2.419, installé dans `D:\Programmes\AutoDesk\Launcher\Revit 2025\`
  (~25 000 fichiers, 4,7 Go). Contenu (bibliothèques, gabarits) dans `C:\ProgramData\Autodesk\RVT 2025\`
  et des gabarits de famille dans `D:\Programmes\AutoDesk\Revit2042\Family Templates\`.
- DLL liées aux fichiers / à la géométrie (taille) : `RevitDB.dll` (44 Mo, base d'objets),
  `PersistenceDB.dll` (7,6 Mo, stockage du fichier), `EssentialsDB.dll`, `FamilyDB.dll`,
  `HostObjDB.dll`, `GeomUtil.dll`, `Graphics.dll`, `OGS*.dll` (affichage), `ASM*230A.dll`
  (noyau de modélisation solide), `Revit.IFC.Export.dll` / `IntfIFC.dll` (export IFC).
- Exemples disponibles : `Samples\*.rvt` (Snowdon Towers ×7, Golden Nugget ×2, 3 autres, 21-134 Mo),
  `RVT 2025\Libraries\**` (~8 300 `.rfa`, quelques `.rvt` de bibliothèque), `DynamoSample_2025.rvt`
  (6,6 Mo), `BatchPrintInitial.rvt` (156 Ko).

## Conteneur (confirmé sur 3 fichiers)

Fichiers lus : `BatchPrintInitial.rvt` (ancien format), `M_Chair-Desk.rfa` (2025), `DynamoSample_2025.rvt` (2025).

- Compound File (MS-CFB). Flux : `BasicFileInfo`, `Contents`, `Formats/Latest`,
  `Global/{ContentDocuments, DocumentIncrementTable, ElemTable, History, Latest, PartitionTable}`,
  `Partitions/NN`, `RevitPreview4.0`, `TransmissionData`, plus `PartAtom` (famille) ou
  `ProjectInformation` (projet). Un seul flux `Partitions/NN` par fichier ici (NN = 1, 70, 283 :
  le numéro ne dit rien de la version).
- `BasicFileInfo` : UTF-16LE `Clé: valeur`, 19 clés en 2025 (`Format: 2025`, `Build: 20240111_1515(x64)`,
  `Locale when saved`, GUID du document…). L'ancien fichier n'a que 5 clés avec `Revit Build`.
  `Last Save Path` contient un chemin utilisateur : à ne pas afficher par défaut (vie privée).
- Pagination du stockage, retrouvée indépendamment : en retirant de chaque flux les octets d'offset
  `o` tels que `o mod 65 249 >= 64 896`, **tous** les membres gzip vérifient leur CRC-32 / longueur
  (1 seul échec sur 349 dans la partition de Dynamo, à examiner : sans doute le dernier membre).
- `Global/*` : 8 octets puis un membre gzip. `Formats/Latest` : gzip à l'octet 0.
  Partition 2025 : en-tête de 44 octets puis membres gzip d'~128 Kio décompressés
  (Dynamo : 6,1 Mo stockés → 27,8 Mo décompressés).

## Où est la géométrie

- Pas de flux ACIS/SAT/SAB dans le fichier (aucune signature trouvée dans la partition décompressée
  de la chaise), et pas de maillage d'affichage en cache évident.
- Le schéma `Formats/Latest` (≈4 600 classes en 2025) nomme les classes de géométrie propre à Revit :
  `GBRep`, `BRep`, `ExternallyTaggedBRep`, `TaggedBReps`, `GFakeBRep`, `GPolyMesh`, `PolyMeshes`,
  `PartialPolyMeshes`, `TessLoop`, `Triangle`, `FaceTag*`, `EmbeddedSymbolGRep`.
  ⇒ La vraie 3D des familles est une B-rep sérialisée par Revit (classes `G*Rep`) dans les partitions.
  Les maillages `GPolyMesh` existent (toposolides, imports DWG/mesh) : piste la plus simple pour un
  premier rendu triangulé.
- `PersistenceDB.dll` (6 877 exports, noms seulement) : classes `RvtFileStorage`, `PartitionTable`,
  `PartitionInfo`, `StreamOpenOptions`, `CSimpleStorage`, `ElementStreamBuilder`, `SegmentIndex`,
  `SegmentAddresses`. Aucun export de compression : la compression est ailleurs (probablement
  bibliothèque statique). Importe `RevitDB.dll`, `Utility.dll`, `ole32.dll` (le stockage CFB).

## État de ModelCraft

`crates/craft-formats/src/rvt.rs` (V4) lit `BasicFileInfo`, la vignette PNG de `RevitPreview4.0`
et l'Atom de `PartAtom`/`ProjectInformation`. Aucune géométrie : `Support::Partial`.

## Inspiration : BIMCraft (26 branches, `main` = la plus complète)

`crates/rvt` (Apache-2.0, en partie porté de rvt-rs, github.com/DrunkOnJava/rvt-rs) : lit le
schéma, `ElemTable`, les enregistrements d'éléments des partitions (catégorie `BuiltInCategory`
à +0x12, boîte englobante 6×f64 en pieds à +0x58 depuis 2024), puis **reconstruit** murs, dalles,
toits, portes/fenêtres, poteaux, pièces à partir des lignes de localisation, esquisses et types
de couches. Pas de lecture des B-rep `G*Rep` : géométrie paramétrique reconstruite, pas le
maillage réel. Sa doc `docs/rvt-provenance.md` liste sources acceptées et interdites (désassemblage
des binaires Autodesk au-delà des noms d'export interdit) - même clôture adoptée ici.
Rien n'est copié dans ModelCraft.

## Prochaines étapes proposées

1. **Lecteur conteneur robuste** dans `rvt.rs` : dépagination (65 249 / 64 896), membres gzip
   bornés, lecture streaming de `Partitions/*` (mémoire bornée), test sur les 3 fichiers + Snowdon.
2. **Schéma** : parser `Formats/Latest` (classes, tags, champs) pour obtenir les tags de
   `GBRep`, `GPolyMesh`, `Outline`, `ElementParents` du fichier lui-même.
3. **Repli « boîtes »** (V5 prévue) : une boîte par élément placé, colorée par catégorie -
   premier affichage 3D réel, faible risque.
4. **GPolyMesh** : localiser ses instances dans les partitions (familles simples, DirectShape,
   toposolides) et décoder sommets/faces.
5. **GBRep** : étudier sur des `.rfa` minuscules (cube, cylindre) générés par l'utilisateur dans
   Revit 2025 (s'il veut bien en enregistrer : sans licence touchée par nous) : faces planes d'abord,
   puis surfaces analytiques ; tessellation côté ModelCraft.
6. Comparaison avec un export IFC fait par l'utilisateur depuis Revit pour valider positions/unités.

## V5 : boîtes englobantes (implémenté, `crates/craft-formats/src/rvt_model.rs`)

Observations faites ici sur les trois exemples (pas de code repris de BIMCraft ni de rvt-rs) :

- La chaîne d'enregistrements au début de la partition : `u64` id, `u32` taille, `u16` constante,
  `u16` compte, corps, 16 octets dont le dernier `u32` répète la taille ; suivant à `o + taille + 16`.
  Dynamo : 6 730 enregistrements = exactement les 6 730 ids de `Global/ElemTable` (id `u64` à +20
  de chaque entrée de 40 octets) ; la chaîne s'arrête à 1,2 Mo sur 38 Mo (le reste : documents des
  familles chargées), donc on arrête la décompression là.
- Constante = 0x05c7 sur tous les enregistrements des deux fichiers 2025 ; + 12 = 0x05d3, la
  valeur lue après la boîte.
- `Formats/Latest` définit `Outline` en ligne : `59 81 00 00 07 00 "Outline"` → tag 0x159
  (bit 0x8000 = définition en ligne). Fichier ancien (BatchPrintInitial) : 0xb1.
- Marqueur `[0x159] ff ff ff ff [0x5d3]` à +0x50 d'une trame, catégorie `i64` à +0x12, genre `u32`
  à +0x42 (`0xffffef7f` = instance placée), boîte 6×`f64` en pieds à +0x58 : **hypothèse
  confirmée** sur 2025. Seules 4 des 38 trames de murs sont des enregistrements de premier niveau :
  les autres sont imbriquées dans le corps d'un enregistrement, d'où la recherche du marqueur dans
  toute la chaîne. Dans une famille (`.rfa`), les solides de la famille ont la catégorie -1.
- Résultats : DynamoSample_2025.rvt → 235 boîtes (38 murs 0,2 m × 2,7 m de haut, 6 sols, 2 plafonds,
  5 portes, 7 fenêtres, 118 meneaux, 36 panneaux, 3 escaliers, 7 garde-corps, 3 poteaux porteurs,
  3 fondations, 3 topographie, 1 volume, 1 générique, 2 entourage) ; M_Chair-Desk.rfa → 4 boîtes
  (assise, dossier, pied, piètement) ; BatchPrintInitial.rvt (format ancien) → 0, repli vignette.
- Filtres : boîte finie, ordonnée, < 3e5 pieds, étendue dans au moins deux directions,
  catégories physiques seulement (lignes, esquisses, niveaux, annotations ignorées), doublons retirés.

## Fichiers de l'utilisateur (sous NDA, jamais versionnés)

Lus en lecture seule sur le PC, `BasicFileInfo` uniquement (aucun nom de fichier ni contenu noté) :
15 `.rvt`, 0,4 à 330 Mo. 14 sont des Revit 2018 (13 en travail partagé « Central », 1 non partagé),
1 est un Revit 2025 « Central ». Le format 2018 n'a pas le cadre d'élément de 2024+ : il reste en
repli vignette/informations ; la piste suivante est le cadre « jusqu'à 2023 » (ids `u32`).
