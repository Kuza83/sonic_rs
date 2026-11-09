#!/usr/bin/env python3
"""Exporte des fichiers lisibles depuis le jeu en marche : écran (PNG), planches de tuiles (PNG, 4 lignes de palette),
palettes (texte), VRAM et RAM de travail (binaires) pour chaque écran/zone. Usage : tools/export_assets.py [dossier]"""
import os, subprocess, sys
out = sys.argv[1] if len(sys.argv) > 1 else 'export'
BIN = './target/release/sonic1_rs'
ZONES = ['green_hill', 'labyrinth', 'marble', 'star_light', 'spring_yard', 'scrap_brain']
sc = {'titre': ('', 600)}
for z, name in enumerate(ZONES):
    for act in range(3):
        sc[f'{name}_acte{act + 1}'] = (f'900:F600:0C,900:FE10:{z:02X},900:FE11:{act:02X}', 3000)
sc['zone_finale'] = ('900:F600:0C,900:FE10:06,900:FE11:00', 3000)
sc['bonus'] = ('900:F600:10', 3000); sc['ecran_continue'] = ('900:F600:14', 3000)
sc['fin'] = ('900:F600:18', 3000); sc['credits'] = ('900:F600:1C', 3000)
for name, (poke, n) in sc.items():
    env = dict(os.environ, NOCPU_POKE=poke, EXPORT_DIR=f'{out}/{name}', EXPORT_EVERY=str(n))
    r = subprocess.run([BIN, '--headless', str(n)], env=env, capture_output=True, text=True)
    ok = os.path.exists(f'{out}/{name}/f{n:05d}_ecran.png')
    print(('ok  ' if ok else 'ECHEC'), name, flush=True)
