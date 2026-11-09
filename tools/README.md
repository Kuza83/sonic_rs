# Outils de test et de comparaison de sonic1_rs

À utiliser depuis la racine du projet (branche `main` ou `dev`), avec `target/release/sonic1_rs` compilé, `sonic1.assets` généré et ta ROM.

- `data_diff.sh ASSETS ROM IMAGES "POKES" [nom]` : exécute le jeu avec la ROM complète puis avec le fichier d'assets (`HASH_EVERY=1`) et signale la première image qui diffère (= octets de données effacés à tort).
- `data_scenarios.sh ASSETS [ROM] [IMAGES]` : lance `data_diff.sh` sur 24 scénarios (zones, actes, bonus, fin, crédits, mort, coup reçu, piques).
- `export_tiles.py [dossier]` : exporte les tuiles distinctes affichées (`donnees/sonic1_pack/originals`, `EXPORT_TILES`).
- `smooth_pack.py DOSSIER N [bicubic|lanczos|nearest]` : fabrique des tuiles agrandies de test (PIL).
- `export_assets.py`, `assets_check.py`, `prov_check.py` : exports lisibles, contrôle du fichier d'assets, contrôle de l'origine des pixels.
- `nocpu_iterate.py`, `nocpu_scenarios.py`, `iterate.sh` : découverte du code non traduit.

Variables utiles du jeu : `NOCPU_POKE="image:adresse:valeur,..."`, `REF_CPU=1` (vrai 68000 émulé, ROM d'origine), `TRACE_RAM=adr:long,...`, `HASH_EVERY=1`, `TRAP_CODE=fichier`, `SONIC_PERF=1`.
