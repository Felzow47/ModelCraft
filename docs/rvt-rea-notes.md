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

### V6.2 : vue de démarrage et ouverture des vues

- Vue de démarrage (Revit « Gérer > Vue de démarrage », API `StartingViewSettings`) : classe
  `InitialViewSettings : Element`, un seul champ `m_initialViewId` (ElementId de la vue ou de la
  feuille). Hypothèse « StartingView » écartée : aucune classe de ce nom dans `/Formats/Latest`
  (3981 classes, Revit 2018 à 2025). L'élément est dans une partition, lu par le même balayage que
  les vues. Absent ou -1 quand le projet n'a pas de vue de démarrage (Revit rouvre alors la
  dernière vue active, non lue).
- Vérifié sur 4 modèles du parc (noms non notés, NDA) : chacun ouvre sur une feuille, à la même
  place dans le navigateur que dans Revit (capture de l'utilisateur), sous une organisation de
  feuilles avec plusieurs niveaux de dossiers.
- `BrowserNode` porte maintenant `id` (ElementId de la vue, -1 sinon) et `start`.
- ModelCraft ouvre le fichier sur cette vue : ligne sélectionnée, dossiers parents dépliés, liste
  défilée jusqu'à elle. Clic sur une vue ou une feuille du navigateur = l'ouvrir : plans (et plans
  de plafond) vus de dessus, coupes / élévations / détails de face, en orthographique ; vues 3D
  en isométrique ; feuilles, vues de dessin, légendes et nomenclatures affichées à plat (feuille
  blanche avec l'aperçu `RevitPreview4.0` quand il existe, qui est souvent la feuille active au
  dernier enregistrement). Pastille « Vue : … » dans la vue 3D, croix pour revenir à toute la
  maquette.
- Pas encore fait : contenu 2D des feuilles (lignes du cartouche, fenêtres, textes), cadrage
  sur la zone de la vue (zone cadrée, plage de vue), filtrage des éléments par vue, orientation
  réelle des coupes / élévations et des vues 3D enregistrées.

## V7 : formes reconstruites (implémenté, `rvt_geom.rs` + `rvt_shape.rs`)

Lecture objet par objet des éléments (décodeur générique piloté par `/Formats/Latest`), puis
reconstruction paramétrique propre (aucune B-rep n'est stockée : `SnapshotData` / `GElement` vides).

Encodage complété (vérifié par égalité taille déclarée / taille consommée) :
- type `b0 b1 00 00` : si `b1 & 0xf0 == 0x10`, un `u32 n` suit (tableau fixe : `10` valeurs,
  `13` n pointeurs bruts de 4 octets, `11`/`12` n pointeurs possédés) ; `50` ensemble / vecteur
  (`51` pointeurs possédés, `53` pointeurs bruts : `u32` compte + n × 4) ; `0d` sous-descripteur
  `[u32 1][0x20][type élément]`. Exemple : les arêtes (`GEdgeBase`, `0e 13` + n) manquaient au schéma.
- pointeur possédé : `u32 v` ; 0 = nul (4 octets) ; sinon `u16` classe (6 octets) ; `0xffffffff` =
  nouvel objet différé (file FIFO) ; petit `v` = identifiant local (première mention : corps en file,
  ensuite : référence) ; `v = 1` document, `v = 2` l'élément lui-même. Pointeur brut (`0e 03`) : 4 octets.

Géométrie lue (pieds, repère projet) :
- **Mur** (`SWall`) : `VWallDriver.m_pCrv` (`GLine` : `m_origin`, `m_dirVec`, `m_endParams`) ;
  profil modifié `VWallExtrusionHelper.m_pCurveLoops` (plan vertical de la ligne de base) ; faces de
  référence `m_pRefFaces` → `Plane` (`m_origin`, `m_xVec`, `m_yVec`, enveloppe `[umin,vmin,umax,vmax]`)
  qui donnent épaisseur, bas, haut et extrémités ; `WallCutoutGStep` / `WallEmbedWallGStep.m_instId`
  = inserts qui découpent (ouverture = boîte de l'insert projetée). Les raccords ne sont pas stockés
  avec le mur : le long d'un axe, la boîte de l'élément les donne.
- **Sol / plafond / dalle** : `m_bottom` / `m_top` (sols), contours `*ExtrusionHelper.m_pCurveLoops`
  horizontaux ; sinon plans de référence horizontaux.
- **Canalisation / gaine / chemin de câbles** (`RbsSingleCurve` et dérivées) : `m_pCurveDriver` →
  `RbsCurveDriver.m_pCrv` (axe), `m_dWidthOrDiameter`, `m_dHeight`, `m_vNormal` (sens de la hauteur),
  `*DesignPropertyManager.m_sOverallSize`.
- **Extrusion de famille** (`ExtrusionElem`) : `m_famId` (document famille), `m_cutting` (vide),
  `m_paramSet` (`ParamValueSetDouble`) : `-1001800` début, `-1001801` fin ;
  `ExtrusionElemExtrusionHelper.m_pCurveLoops` ; plan d'esquisse `VarSketch.m_pPlane`.
- **Instance** (`FamilyInstance`) : `m_pInstanceInfo` → `InstanceInfo.m_Trf` (`m_3x3`, colonnes =
  images de X, Y, Z ; `m_or`) et `m_symbolId` ; `FamilySymbol.m_famId` = document famille intégré
  (sinon `m_familyId` / `m_partitionSurrogateId` : famille chargée dont la géométrie n'est pas dans
  le projet, seule la boîte reste). Contrôle : chaque forme placée doit rester dans la boîte de
  l'élément (sinon boîte).

Validation (échantillons publics + fichier sous NDA, localement) : sur 5 660 formes reconstruites du
fichier MEP sous NDA, 4 s'écartent de plus de 5 cm de la boîte Revit ; `M_Chair-Desk.rfa` : 4/4.
Pas encore : murs courbes, toits, lissages / balayages / révolutions, raccords MEP (familles chargées).

### Solides des familles chargées (révolutions, balayages, lissages, familles imbriquées)

- Documents de famille stockés dans le projet : `/Global/ContentDocuments` (segments : balise
  ContentMarker, GUID ContentKey, puis id racine après la balise NOBLE_SecondaryDataStorage). Les
  éléments du document ont `m_famId` = racine. `Family.m_oFamDoc` : `m_contentDocGUID`,
  `m_big2SmallMap2` (ids projet → ids du document : le propriétaire d'un id « first » donne la racine).
- `FamilySymbol` : (`m_famId`, `m_familyId`) ; symboles imbriqués : `m_famId` = racine du document
  qui les contient (ne pas s'en servir comme famille : auto-référence).
- `RevolutionElem` : esquisse `m_sketchId`, angles -1001802 / -1001803. `SweepElem` :
  `m_profileData.m_profileSketchId` + `m_pathSketchId`, ou `m_path3dId` → `Path3d`
  (`m_curveLoops` → `CurveLoop.m_curves`, `m_userId` = le balayage) : c'est le cas des coudes MEP.
  `BlendElem` : deux esquisses dont `m_userId` = le lissage, hauteurs -1001804 / -1001805.
- Formes ignorées : paramètre entier -1006205 (« Visible ») = 0, ou présence d'un
  `AddGeomToHostInFamEnvGStep` (hôte d'exemple 8 pi × 8 pi des gabarits hébergés). Les drapeaux 9 / 11
  de `GeomStepList` ne distinguent pas l'hôte.
- Placement : par solide ; on garde les solides dont la boîte placée tombe dans la boîte Revit de
  l'instance s'ils la couvrent (≥ 60 % de chaque côté ≥ 10 cm), sinon mise à l'échelle uniforme autour
  de l'insertion (rapport médian 0,1 à 10, côtés à 8 % près), sinon étirement par axe (placements
  alignés sur les axes, 0,33 à 3). Le type peut avoir d'autres cotes que la famille enregistrée :
  c'est une approximation.
- Validation (fichier MEP sous NDA) : 9 721 formes (5 660 avant), 513 à plus de 5 cm de la boîte
  Revit, 9 à plus de 30 cm. Raccords de canalisation 3 177 / 6 140, raccords de gaine 132 / 1 191, accessoires de canalisation
  398 / 586 ; sprinkleurs et gaines souples : aucun.


## Feuilles (contenu 2D) et cadrage des vues

Lu par `crates/craft-formats/src/rvt_sheet.rs`, dans la même passe que le navigateur et les formes.

- Éléments possédés par une feuille : `m_ownerDBViewId` = id de la `DBViewSheet`.
- Cartouche et symboles 2D : `FamilyInstance` (`m_pInstanceInfo.m_Trf` : `m_3x3`, `m_or`, pieds),
  symbole `FamilySymbol` : courbes `m_nonBRepGeomPtrs` (toutes à z = 0, `m_outline.m_minmax` plat),
  textes `m_textNoteData`, étiquettes `m_tagNoteData` (valeur = paramètre de la feuille, du projet
  `ProjectInfo` ou intégré : -1007400 nom, -1007401 numéro, -1007402 échelle), tableaux
  `AllScheduleInstanceRegenData` (tableau des révisions).
- Textes : `TextNote` (texte riche RTF réduit en texte simple, `\'hh` Windows-1252, `\uN`) ;
  `m_height` = boîte : lignes × taille × ~1,57.
- Vues placées : `Viewport` (`m_offset` centre sur la feuille, `m_scale`, `m_dbViewId`,
  `m_labelOrigin` / `m_labelLength` ligne de titre, `m_detailNumber`). Zone cadrée :
  `Viewer.m_boundedSpace` (`m_orig`, `m_basis` 3×3 = axes u, v, profondeur, `m_boundOffset` 6 :
  u max, u min, v max, v min, loin, près ; `m_boundActive`). Les coupes, élévations
  (`Section`, `InteriorElevArrow`…) sont des sous-classes de `Viewer` : tester `isa("Viewer")`.
  La vue 3D a la classe `DBView3d`. Plage de vue des plans (`DBView`, pieds) : valeurs absolues
  `m_cutPlaneElev`, `m_topClipElev`, `m_bottomClipElev`, `m_depthClipElev` quand elles sont
  remplies, sinon `PlanViewRange2.m_offsets` (coupe, haut, bas, profondeur) relatifs à
  `m_levelPos` (0 = niveau de la vue `m_genElemId`, 1 = niveau au-dessus, -1 = illimité ; -1
  l'emporte sur une valeur stockée à 0, vu sur un plan de masse). L'ancien `PlanViewRange` peut
  être tout à 0. Plan regardant vers le haut (plan de plafond, `m_viewDir.z < 0` ou profondeur
  au-dessus de la coupe) : de la coupe à la profondeur ; sinon de min(profondeur, bas) à la coupe.
  Coupes : la coupe est toujours au trait (`off[5]` appliqué), la découpe lointaine `off[4]`
  seulement si active. Zone de coupe 3D : `ModelClipBox` (`m_isOn`, `m_viewer3dId` → `Viewer`
  de la vue 3D, `m_boundedSpace`). `ViewFrame.clip` : boîte orientée (axes, bornes, ±1e7 pour
  illimité) appliquée par le shader (masquage + fermeture des faces coupées) et par la sélection,
  et utilisée pour couper les arêtes des vues placées sur les feuilles (Liang-Barsky 3D). Images des rendus : `ImageInstance` → `ImageSymbol` (`m_widthInFeet`, `m_heightInFeet`)
  → `ImageHolder.m_pRasterImage.m_compressedImage` (PNG / JPEG / BMP / GIF).
- Annotations des vues (`rvt_annot.rs`, éléments du projet `m_famId = -1` dont
  `m_ownerDBViewId` est la vue ou la feuille) :
  - lignes de détail : `CurveElem` (`m_detail = 1`), courbe `m_pCurveDriver` →
    `CurveElemDriver.m_pCrv` (`GLine`, `GArc`… en pieds, coordonnées du modèle) ; style
    `m_categoryId` → `GStyleElem.m_pGStyle` (`m_penNumber`, `m_color` rouge dans l'octet bas,
    `m_linePatternId` : -3000010 trait plein, -1 « lignes invisibles », sinon `LinePatternElem` →
    `LinePattern.m_segs` : `m_type` 0 trait, 1 espace, 2 point, `m_data` longueur papier en pieds).
    Les styles utilisateur sont des sous-catégories de -2000051 (`CategoryElem.m_parentCategoryId`).
  - plumes : `PenWidthTableElem` (id 2) → `PenWidthTable.m_draftPenInfo.m_pens` (16 épaisseurs
    papier en pieds, annotations) ; `m_modelPenInfo` par échelle (`m_invertedScale`) pour le modèle.
  - régions remplies : `FilledRegion` (`m_sketchId`, `m_attrId` → `FilledRegionAttributes.
    m_fillPatternElemId` → `FillPatternElem.m_pFillPattern`) ; contour = les `CurveElem` dont
    `SketchMembership.m_groupId` = `m_sketchId` (données `FilledRegionCurveData`, souvent en lignes
    invisibles). `FillPattern` : `m_name`, `m_scale`, `m_fpTarget` (0 dessin, tailles papier),
    `m_gridsArr` → `FillGrid` (`m_angle` rad, `m_origin`, `m_deltas` = [décalage le long, espacement],
    `m_segs` traits / espaces) ; sans grille = remplissage plein. Hachures découpées pair-impair.
  - cotes linéaires : `LinearDimString` (`m_refPnts` liste xyz à plat, ligne de cote = l'objet
    `GLine` de l'arbre `m_origin` / `m_dirVec`, `m_ArrSegInfo[].m_origin` position du texte de
    chaque segment, `m_values[0].m_value` valeur en pieds) ; longueur recalculée par projection
    des points sur la ligne. Unité : `UnitsElem`, entrée `autodesk.spec.aec:length-1.0.0` →
    `m_unitTypeId.m_typeId` (`autodesk.unit.unit:millimeters-1.0.1`…) et `m_accuracy`.
  - étiquettes : `IndependentTag` (`m_symbolId`, `m_oTagOrientationCell` → `TagOrientationCell.
    m_headLocation` (modèle), `m_rotationAngle` ; élément étiqueté `TaggedElem.m_taggedId.
    m_linkInstOrHostId`), `RoomTag` (`m_point`, `m_symbolId`, `m_taggedRoomElemId.
    m_linkInstOrHostId`). Le symbole est une `FamilySymbol` 2D (tailles papier) dont les
    `m_tagNoteData` nomment les paramètres : pièce -1006900 nom, -1006901 numéro (chaînes de
    `RoomElem`), -1006902 surface (calculée, non stockée).
  - vues de dessin : classe `DBViewAnonDrafting`, zone cadrée par défaut (±100, inactive) : pas
    de modèle, cadre = étendue des annotations.
- Direction des vues (`DBView`) : `m_viewDir` vers l'observateur (comme `View.ViewDirection` de
  l'API), `m_vertDir` haut de l'écran, `m_horzDir` droite ; vérifié sur les élévations Nord (+y) /
  Sud (-y) et les plans (+z). `Scene.view_frames` : direction, haut et boîte de la zone cadrée ×
  profondeur (bornée à la maquette) ; la visionneuse en tire lacet / tangage de l'orbite type
  Blender (plans pivotés : lacet tiré du haut).

### Lignes cachées des vues placées (`rvt_hlr.rs`)

Pas de donnée Revit : calcul fait par ModelCraft. Repère de la vue : u = `m_horzDir` (ou axe x
de la zone cadrée), v = haut, w = `m_viewDir` (vers l'observateur ; l'axe z de la zone cadrée
d'une coupe pointe à l'opposé, la profondeur lointaine `off[4]` y est positive). Les faces des
formes reconstruites (pas les boîtes englobantes : murs-rideaux, escaliers, pièces y seraient
pleins) sont découpées par ce que garde la vue (zone cadrée, plage de vue, coupe 3D), puis
dessinées dans un tampon de profondeur (2 400 px sur le grand côté). Une arête garde les
parties devant la face la plus proche (minimum des 3 × 3 pixels voisins, tolérance 0,02 pi
+ ¼ pixel). `RVT_NO_HLR=1` : toutes les arêtes.

### Éléments et catégories masqués par vue

Dans l'enregistrement de la vue (`DBView…`) : `HiddenElementsViewSettings.m_hiddenElements`
(ids des éléments masqués dans la vue) et, parmi les `m_oaDrawFilters`,
`IckyExcludedCategoriesSetPtrWrapper.m_categoryIds` (catégories décochées dans la visibilité de
la vue, ids négatifs des catégories intégrées) avec `m_analyticalModelsExcluded`,
`m_modelsExcluded`, `m_annotationsExcluded`. Exemple public rst_2023, vue 3D « Central Pile »
sans zone cadrée ni zone de coupe : seules les fondations restent, le reste de la structure est
dans des catégories masquées. Appliqué aux vues placées sur les feuilles (pas encore à la vue 3D
interactive, dont les composants regroupent plusieurs éléments).
`ExtentElem` (`m_extentElemId` de la vue) : `m_idExtents` vide sur les échantillons.

## Cotes radiales, angulaires et altimétriques

- `RadialDim` : `m_center` = centre de l'arc, `m_refPnts[0]` = point sur l'arc,
  `m_ArrSegInfo[0].m_values[0].m_value` = rayon (pieds), `m_ArrSegInfo[0].m_origin` = position du texte.
  Dessin : trait du centre vers le point (prolongé jusqu'au texte), flèche sur l'arc, texte « R <rayon> ».
- `AngularDim` : deux points sur l'arc de cote dans `m_refPnts`, centre et rayon dans l'objet `GArc`
  associé, `m_planeNormal` = plan de la cote, valeur en radians. Précision lue dans `UnitsElem`
  (spec `autodesk.spec.aec:angle-1.0.0`). Aucun exemple public n'en contient dans une vue de projet :
  non validé visuellement.
- `SpotElevation` : `m_refPnts` = point coté, `m_bendOrPosPtOffset` = coude de la ligne de repère,
  `m_horizLeaderLength` = épaulement, valeurs = [altitude, est, nord]. La présence d'un objet
  `SpotCoordinateState` signale une coordonnée ponctuelle (texte « N … » / « E … »).
  Vérifié sur rst_2023 S101 (E 13062 / N 2799).

## Cadrage des annotations

- Porté par l'élément `Viewer` de la vue (et non par `DBView`) : `m_bAnnotationCropActive` et
  `m_pAnnotationCropOffsets` → objet avec `m_leftRightTopBottomOffsets` (gauche, droite, haut, bas),
  en pieds **papier** au-delà de la zone cadrée (valeur par défaut 0,0833 = 25,4 mm).
- Comme dans Revit, une annotation entièrement hors du cadre n'est pas dessinée ; une annotation qui
  le touche reste entière (pas de découpe). Appliqué seulement aux vues cadrées.
- Sur les exemples publics toutes les annotations sont déjà dans le cadre : aucun changement visible.

## Styles de cotes et repères de fin

- Une cote (`LinearDimString`, `RadialDim`, `AngularDim`) pointe son type par `m_styleSymbolId`
  → `DimensionStyle`. Celui-ci donne le repère de fin `m_arrowHeadStyleId` → `LeaderStyle`, et la
  taille du texte dans le paramètre double -1006433 (pieds papier ; 0,0078 = 3/32", 0,00787 = 2,4 mm).
  Autres paramètres vus (-1006465, -1006431, -1006407, -1006405, -1006404, -1006401) : non identifiés
  avec certitude (écarts et prolongements des lignes d'attache), non utilisés.
- `LeaderStyle` : `m_tickType` 0 = trait oblique, 8 = flèche ; `m_arrowFilled`, `m_arrowClosed` ;
  paramètre -1006426 = angle d'ouverture de la flèche (radians, 0,5236 = 30°), -1006414 = taille
  (pieds papier ; « Diagonal 3mm » = 0,00984).
- Dessin : trait oblique à 45° de la taille donnée ; flèche pointe sur la ligne d'attache, corps vers
  l'intérieur du segment (deux flèches dos à dos sur les attaches intermédiaires), pleine, fermée ou
  ouverte selon le type.

## Lignes de coupe

- Le plan de coupe d'une vue est la borne de la boîte conservée du côté de l'observateur
  (plans : hauteur de coupe de la plage de vue ; coupes et élévations : leur ligne). Les maillages
  reconstruits qui le traversent y sont sectionnés (intersection triangle/plan) et le contour est
  dessiné en trait épais (0,35 mm), jamais masqué.
- Les catégories que Revit ne coupe pas (mobilier, appareils sanitaires, luminaires, équipements,
  plantes, entourage, stationnement, site, topographie) restent en projection.
- Limite : l'épaisseur ne vient pas encore des styles d'objet du projet (les catégories intégrées
  n'ont pas de `GStyleElem` dans le fichier ; leurs styles restent à localiser), une seule
  épaisseur de coupe est utilisée, les lignes en projection restent fines.

## Ossature, meneaux et panneaux de mur-rideau

Les poutres et contreventements (`FamilyInstance` avec `m_pCurveDriver` → `FamInstCurveDriver`)
portent leur ligne de positionnement : `m_pCrv` → `GLine` (`m_origin`, `m_dirVec`, `m_endParams`),
avec les retraits `m_setbacks` aux deux extrémités. Quand les solides de la famille ne sont pas
retrouvés (profils paramétrés par la longueur de l'instance), on construit une boîte le long de
cette ligne : la largeur et la hauteur de la section sont résolues (moindres carrés) à partir de la
boîte englobante de l'élément, chaque étendue monde valant `L·|n_i| + w·|y_i| + h·|z_i|` ; la section
est centrée sur la boîte englobante (décalages de justification), la longueur sur la ligne.

Les meneaux (`MullionFamRef`) et panneaux (`CurtainCellFamRef`) sont des boîtes dans les axes de
leur `m_Trf` : les arêtes résolvent `e_i = Σ_j |u_j[i]| · l_j` (Cramer), refusé si le système est
dégénéré (rotation à 45°) ou une arête négative.

Limites : section rectangulaire seulement (pas de profil en I ni de tube), pas de coupes d'onglet,
meneaux d'angle et panneaux non rectangulaires restent approchés.
