# Projets étudiés pour inspiration (salle blanche)

ModelCraft V4 s'est inspiré du **comportement** et des **idées** de deux projets open source
indiqués par l'utilisateur. Règle suivie : lecture pour comprendre ce qu'ils font et comment ils
s'organisent, puis réécriture complète en Rust par nous. **Aucune ligne de leur code n'a été
copiée** dans ModelCraft (ni de rvt-rs, dont BIMCraft dérive en partie). Rien non plus d'eDrawings,
SolidWorks ou Autodesk.

## tangency-labs/solidcraft

- **Nature** : CAO paramétrique en Rust / egui (esquisses, contraintes, fonctions). Pas de lecture
  SLDPRT/SLDASM, ni des flux d'affichage SolidWorks ; imports STEP/IGES/3MF/STL/OBJ/glTF.
- **Idées retenues (réimplémentées à notre façon)** :
  - préréglages de navigation (style Blender / CAO) et raccourcis documentés dans l'application ;
  - double-clic molette = tout cadrer ; rotations de vue par pas de 15° au clavier ;
  - Ctrl+Z / Ctrl+Y sur l'état de la vue ;
  - vue éclatée animée ;
  - vrai installateur Windows avec menu Démarrer.
- **Non repris** : son modèle paramétrique (hors sujet pour une visionneuse), son installateur MSI
  (nous utilisons Inno Setup, généré depuis notre propre liste d'extensions).

## IPedrax/BIMCraft (toutes les branches)

- `main` est la branche la plus complète. Les 25 autres branches contiennent des versions
  mineures ou plus anciennes de `crates/rvt`.
- **Lecteur RVT** (Apache-2.0, en partie porté de rvt-rs) : reconstruction **sémantique** des
  éléments (niveaux, murs, dalles/plafonds, toits, portes/fenêtres, poteaux, pièces), pas de
  maillage Revit natif. Familles, escaliers et groupes sont ignorés.
- **Faits de format notés** (faits, pas du code) : à partir de Revit 2024, un enregistrement
  d'élément porte une boîte englobante de 6 `f64` (en pieds) à +0x58 et un `BuiltInCategory`
  (`i64`) à +0x12.
- **Piste V5 (non faite)** : un repli « boîtes d'éléments » pour les RVT, écrit par nous à
  partir de ces faits, en restant en lecture streaming et mémoire bornée.

## Ce qui en est sorti dans la V4

| Idée | Implémentation ModelCraft |
|---|---|
| Préréglages de navigation | `NavStyle` Blender (défaut) / SolidWorks / CAO, réglage « Raccourcis Blender » |
| Pas de 15° au clavier, vue opposée | `view.orbit_*` (pavé 4/6/8/2), `view.flip` (pavé 9) |
| Double-clic molette = tout cadrer | `viewer.rs`, gestion souris |
| Annuler / rétablir | pile `ViewState` dans `document.rs` (positions, éclaté, opacités, masquage) |
| Éclaté animé | `assembly.rs` : une direction radiale fixe par pièce, jamais inversée |
| Installateur | `installer/gen_iss.py` → `installer/modelcraft.iss` (Inno Setup) |
