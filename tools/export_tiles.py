#!/usr/bin/env python3
"""Exporte toutes les tuiles distinctes affichées pendant les scénarios (dossier/originals/<hash>_<palette>.png, 8x8).
Usage : tools/export_tiles.py [dossier]   (lancer avec SONIC_ASSETS=... ; relançable : les tuiles déjà vues sont réécrites à l'identique)"""
import os, subprocess, sys
out = sys.argv[1] if len(sys.argv) > 1 else 'donnees/sonic1_pack'
BIN = './target/release/sonic1_rs'
sc = {'titre': ('', 1500), 'long': ('', 12000)}
def sweep(z, act):
    # Zone choisie avant le chargement du niveau (image 740, juste après Start) puis Sonic téléporté le long du niveau
    # pour afficher beaucoup de décors, d'ennemis et de boss (2 400 images de balayage).
    it = [f"740:FE10:{z:02X}", f"740:FE11:{act:02X}"]
    f = 1400
    for x in range(0x200, 0x2A00, 0x180):
        for k in range(0, 150, 3): it += [f"{f+k}:D008:{x>>8:02X}", f"{f+k}:D009:{x&255:02X}", f"{f+k}:FE21:30"]
        f += 150
    return ','.join(it)
for z in range(6):
    for act in range(3):
        if z == 5 and act > 0: continue
        sc[f'zone{z}-{act}'] = (sweep(z, act), 4000)
sc['bonus'] = ('900:F600:10', 3500); sc['continue'] = ('900:F600:14', 3000); sc['fin'] = ('900:F600:18', 5000); sc['credits'] = ('900:F600:1C', 5000)
sc['demo'] = ('900:F600:08', 4000)
from concurrent.futures import ThreadPoolExecutor
def run(item):
    name, (poke, n) = item
    env = dict(os.environ, NOCPU_POKE=poke, EXPORT_TILES=out)
    subprocess.run([BIN, '--headless', str(n)], env=env, capture_output=True, text=True)
    cnt = len(os.listdir(f'{out}/originals')) if os.path.isdir(f'{out}/originals') else 0
    print(f'{name:12} -> {cnt} tuiles distinctes au total', flush=True)
os.makedirs(f'{out}/originals', exist_ok=True)
with ThreadPoolExecutor(4) as ex: list(ex.map(run, sc.items()))
