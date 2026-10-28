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

## V5.1 : fichiers d'avant 2024 (identifiants 32 bits)

Vérifié sur les projets d'exemple publiés par Autodesk (revit.downloads.autodesk.com, non versionnés) :
`rst_basic_sample_project` 2018, `rstbasicsampleproject` 2023 et 2025, `rac_basic_sample_project`
2018 et `racbasicsampleproject` 2021.

- Revit 2024 est passé aux `ElementId` 64 bits. Avant : en-tête d'enregistrement `u32` id, `u32`
  taille, `u16` constante, `u16` compte (12 octets), bloc de fin de 12 octets dont le dernier `u32`
  répète la taille ; enregistrement suivant à `o + taille + 12` (contre `+ 16` en 2024+).
- Le marqueur `[Outline] ff ff ff ff [tag]` reste, suivi des six `f64` ; tag `Outline` = 0x109 (2018),
  0x13c (2023), 0x159 (2025), lu dans `Formats/Latest`. Le tag qui suit n'est plus « constante + 12 »
  en 2018 (0x4f4 pour une constante 0x4eb) : on prend le plus fréquent après le marqueur.
- Type de placement (`u32`) toujours 14 octets avant le marqueur ; catégorie `i32` 0x26 octets avant
  (0x3e et `i64` en 2024+ : les six identifiants intermédiaires sont deux fois plus courts).
- Même modèle, mêmes résultats d'une version à l'autre : rst 786 boîtes en 2018, 2023 et 2025 ;
  rac 458 en 2018 et 2021.
- Depuis 2019, le bloc `Clé: valeur` de `BasicFileInfo` est à un décalage impair : on prend
  l'alignement qui contient les clés.
- Les fichiers en travail partagé sont lus comme les autres (une partition par sous-projet), en
  lecture seule : aucun verrou, aucun fichier temporaire, aucune synchronisation avec le central.

## V6 : navigateur de projet (implémenté, `crates/craft-formats/src/rvt_browser.rs`)

Objectif : à l'ouverture d'un .rvt, le panneau de gauche montre le « Navigateur de projet » comme
Revit (Vues par dossiers, Légendes, Nomenclatures/Quantités, Feuilles), en lecture seule.
Observations sur les échantillons publics uniquement (2018, 2021, 2023, 2025), sans
désassemblage.

**Schéma (`/Formats/Latest`).** Chaque classe : `[u16 len][nom][u16 parent]` (si le parent a le
bit 0x8000 : `00 00` puis sa définition en ligne), `[u32][u32 nb champs]`, les champs
`[u32 len][nom][type]`, puis `u32 ng` et ng GUID de 16 octets. Type : `b0 b1 00 00`, puis
`u32 n` si b1 = 0x10, puis `u16 classe` si b0 = 0x0e et b1 ∈ {0x00, 0x50, 0x10}.
`0e 00 c` objet par valeur (c = ElementId : un id de 4 ou 8 octets), `0e 01/02` pointeur
possédé, `0e 03` pointeur brut (u32), `0e 50 c` ensemble (u32 n puis n éléments),
`0e 51` tableau de pointeurs, `08 60` chaîne UTF-16 (u32 longueur), `07` double, `04` int32,
`01` booléen, `0b` 8 octets (2025, `TreeItemData`).

**Éléments dans les partitions.** `[id][u32][u32 taille][u16 classe]`, le corps suit
directement la classe ; champs de toute la chaîne de classes dans l'ordre du schéma (Element
d'abord : `m_id`, `m_famId`, `m_ownerDBViewId`…). Pointeur non nul : `ff ff ff ff [u16 classe]`
et son contenu est écrit après le corps, dans l'ordre de rencontre (file) ; `00000000` nul ;
petite valeur = renvoi. On valide un élément quand `m_id` décodé = id de l'en-tête. Les vues
sont au-delà de la fin de la chaîne d'enregistrements : on lit les partitions entières.

**Ce qu'on lit.** `DBView*` (nom, type, gabarit, vue principale pour les vues dépendantes,
fenêtres sur feuille, numéro de feuille), leurs jeux de paramètres différés
(`ParamValueSetInt/ElementId/AString/Double` : discipline -1005163, phase -1012102…),
`DBViewType` (`m_systemFamilyIdx` : 0x66 3D, 0x68 rendu, 0x69 nomenclature, 0x6b feuille,
0x6c dessin, 0x6d plan d'étage, 0x6e surface, 0x6f plafond, 0x70 coupe, 0x71 détail,
0x72 élévation, 0x75 légende, 0x76 tableau, 0x77 poteaux graphiques, 0x78 structure ; nom
du type dans `SymbolInfo`), `ProjectPhase`, `BrowserOrganization` (filtres, définitions de
dossiers `{paramètre, nb de caractères}`, tri ; `m_type` 0 vues, 1 feuilles, 3
nomenclatures) et, dans `/Global/Latest`, `BrowserOrganizationTracking` (organisation
courante des vues, feuilles et nomenclatures ; sa table `m_folderNameToIdMap` contient les
noms de dossiers de Revit, qui ont servi à vérifier la règle de nommage).

**Arbre.** Exclus : gabarits, vues de familles (`m_famId` ≠ -1), vues possédées par une feuille
(nomenclatures de révisions), `DBViewProject`, navigateur de systèmes. Dossier « famille et
type » : pluriel de la famille, plus « (type) » si le nom du type n'est pas le nom par
défaut (« Coupes (Wall Section) », « Plans d'étage »). Les élévations d'avant 2019 sans type
prennent le premier type de la famille élévation. Valeur vide : dossier « ??? ». Tri
alphanumérique sans accents ; vues dépendantes sous leur vue principale ; feuilles « NUM -
Nom ». Filtres : égal / différent seulement (« pas sur des feuilles » pris en charge).

**Reste inconnu.** Aucune légende ni paramètre de projet dans les échantillons (valeurs
attendues dans `ParamValueSetAString` avec des id positifs) ; libellés des valeurs
ElementId autres que les phases ; relations de filtre autres qu'égal/différent.

### V6.1 : familles, groupes, liens Revit, mode simplifié

- Familles chargeables : `FamilySurrogate` (`m_name`, `m_categoryId` catégorie intégrée,
  `m_elemId` = élément `Family`) et `FamSymSurrogate` (`m_name` du type, `m_famSurrogateId`).
  Les familles in situ (`Family.m_inPlace`) sont exclues ; une même famille peut avoir plusieurs
  substituts (fusionnés). Les noms de catégories ne sont pas stockés (localisés par l'interface) :
  table des catégories intégrées courantes, annotations regroupées sous « Symboles d'annotation »,
  le reste sous « Autres familles ». Les familles système (murs, canalisations…) ne sont pas encore
  listées.
- Groupes : `ElementGroupType` (nom dans `SymbolInfo`), détail si `m_viewType != 0` ou attaché
  (`m_attachedParentId`, rangé sous son groupe de modèle). Hypothèse à confirmer sur un modèle
  avec groupes de détail.
- Liens Revit : `RvtLinkSymbol` (nom `SymbolInfo.m_name`, liens imbriqués exclus via
  `m_parentSymbolId`), `RvtLinkInstance` (`m_name` = numéro d'instance, type via
  `InstanceInfo.m_symbolId`). État au dernier enregistrement : flux `TransmissionData`
  (u32 + XML UTF-16, `ExternalFileReference` de type « Revit Link », `LastSavedLoadState`
  Loaded / Unloaded / NotFound…). Lien non chargé = icône rouge.
- Décodeur : types `0x09` (GUID, 16 octets), `0x0d` (XYZ, 24 octets) et tableaux fixes d'objets
  `0e 10`.
- Interface : bouton près de l'arbre pour basculer entre le navigateur complet (comme Revit) et la
  vue simplifiée (vues, légendes, nomenclatures, feuilles) ; préférence `browser_full` enregistrée
  dans settings.json.
- Catégories : identifiants vérifiés contre la liste publique `BuiltInCategory` de l'API Revit
  (valeurs stables entre versions). Toutes les catégories d'étiquettes, têtes, symboles, repères,
  titres de vue et conditions d'appui sont rangées sous « Symboles d'annotation ». Sur le parc local,
  il ne reste plus de catégorie inconnue parmi celles relevées.
