# sonic_rs — Sonic the Hedgehog (Mega Drive) en Rust

Version Rust de Sonic 1 : le code 68000 de la ROM est recompilé statiquement en Rust et exécuté sans processeur émulé ;
seules les puces vidéo et sonores (VDP, YM2612, PSG, Z80) viennent d'un émulateur. Rendu haute résolution (×2 à ×6, filtre Scale3x,
remplacement de tuiles par des versions améliorées).

Cette branche `main` est réservée aux versions publiées (release) : elle ne contient pour l'instant que ce fichier.

## Branches
- `dev` : développement en cours (code du jeu, recompilateur, runtime, copie de `md-core`, outils de test).
- `tools` : uniquement les outils de test et de comparaison.

Aucune donnée de Sega (ROM, fichier d'assets, tuiles) n'est dans ce dépôt : il faut posséder sa propre ROM de Sonic 1.
