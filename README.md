# sonic1_rs — Sonic the Hedgehog (Mega Drive) en Rust

Le code 68000 de la ROM est recompilé en Rust (`gen/`, généré, ne jamais éditer à la main) et exécuté par un runtime
sans CPU émulé (`rt/`). `md-core` (copie dans `vendor/megadrive-rs`, avec le suivi d'origine des pixels ajouté au VDP) ne fournit que le matériel : VDP, YM2612, PSG, Z80.

## Jouer
```
cargo build --release
./target/release/sonic1_rs --make-assets sonic1.assets      # une fois : seule étape qui lit la ROM
./target/release/sonic1_rs       # jouer (ou SONIC_ROM=chemin.md pour la ROM directe)
```
Clavier : flèches, W=A, X=B, C=C, Entrée=Start, Échap=quitter. Manette : A→B, B/R1→C, X/Y/L1→A, Start/Back→Start, croix + stick gauche.

## Contenu
- `src/main.rs` point d'entrée ; `src/host.rs` fenêtre/son/manette/boucle ; `src/devtools.rs` outils ; `src/recomp.rs` recompilateur.
- `rt/` runtime (machine, instructions, moteur sans CPU) ; `gen/` code généré (~12 min à compiler).
- Outils : `--headless N [out.ppm]` (N images sans fenêtre, empreinte toutes les 60 images ; `NOCPU_POKE="image:adresse:valeur,..."`),
  `--make-assets`, `--recompile`, `--sweep`, `--flow ADR`, `--disasm ADR LONG`.
- `regen.sh` régénère `gen/` à partir des graines (`seeds*.txt`) ; `tools/` : découverte de code non traduit
  (`nocpu_iterate.py`, `nocpu_scenarios.py`, `iterate.sh`) et validation des assets (`assets_check.py`, `assets_keep.txt`).

## Haute résolution
Principe : le VDP de md-core enregistre l'origine de chaque pixel (tuile, position, retournements, palette, couche) ; `src/hd.rs` refait
l'image à N× en remplaçant chaque pixel par le bon morceau de la tuile améliorée, ou à défaut en l'agrandissant en bloc.
1. Exporter les tuiles distinctes (une fois, ~5 min) : `python3 tools/export_tiles.py donnees/sonic1_pack`
   → `donnees/sonic1_pack/originals/<hash>_<palette>.png` (8×8, ~9 700 tuiles).
2. Fournir les versions améliorées dans `donnees/sonic1_pack/tiles/` : mêmes noms, PNG RGB/RGBA 8 bits, carrés de 8N×8N (N de 2 à 6, identique pour toutes).
   Les tuiles absentes sont simplement agrandies. Test sans amélioration : `./sonic --upscale-pack donnees/sonic1_pack 3`.
3. Jouer : `SONIC_PACK=donnees/sonic1_pack ./sonic` (l'échelle est celle des tuiles ; `SONIC_SCALE=N` sans lot = agrandissement simple).
   Capture : `--headless N sortie.png` avec SONIC_PACK ou SONIC_SCALE sauvegarde l'image agrandie.
Couleurs : affichée = couleur actuelle + (tuile améliorée − son original agrandi), donc l'eau, les fondus et les palettes animées s'appliquent aussi.
Contrôles : `PROV_CHECK=1`/`tools/prov_check.py` (origine des pixels), `HD_CHECK=N` (+ `HD_ORIENT=1` : orientation des retournements ; avec SONIC_PACK : couverture du lot).
Limite : chaque tuile est améliorée isolément (8×8) : un agrandisseur automatique ne voit pas les tuiles voisines, des coutures peuvent apparaître.

## Fichier d'assets
La ROM dont le code traduit est effacé (85 271 octets sur 524 288) ; somme de contrôle de l'en-tête recalculée.
`assets_keep.txt` : octets de code que le jeu lit comme données (213), conservés. `TRAP_CODE=fichier` relève ces lectures.
`tools/assets_check.py` compare les images ROM/assets sur 25 scénarios.

## Retiré lors du nettoyage
Mode hybride (CPU émulé + interceptions), `--verify`, `--fuzz`, `--diff`, 75 routines écrites à la main (remplacées par le code généré).
Elles avaient servi à valider le recompilateur ; leurs adresses sont gardées dans `seeds_known.txt`.

## Dépôt et branches
- `main` : le jeu (src, rt, vendor, recompilateur). `dev` : développement en cours (main + outils). `tools` : uniquement les outils de test (`tools/`).
- **Aucune donnée de Sega n'est dans le dépôt** (ni ROM, ni `sonic1.assets`, ni tuiles exportées). `gen/src/lib.rs` n'y est qu'un squelette vide :
  après un clone, mettre ta ROM (`SONIC_ROM=chemin.md` ou chemin par défaut dans `src/host.rs`), puis `./regen.sh` (~12 min) génère le vrai code,
  puis `./target/release/sonic1_rs --make-assets sonic1.assets`.
- En local, `git update-index --skip-worktree gen/src/lib.rs` évite de voir le code généré comme modifié.
