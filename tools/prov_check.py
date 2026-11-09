#!/usr/bin/env python3
"""Vérifie l'enregistrement d'origine des pixels (PROV_CHECK) sur les scénarios : tuiles et couleurs reconstruites = image produite."""
import os, subprocess, re
BIN = './target/release/sonic1_rs'
sc = {'titre': ('', 1500), 'long': ('', 9000)}
for z in range(6):
    for act in range(3):
        sc[f'zone{z}-{act}'] = (f'900:F600:0C,900:FE10:{z:02X},900:FE11:{act:02X}', 3000)
sc['bonus'] = ('900:F600:10', 3000); sc['continue'] = ('900:F600:14', 3000); sc['fin'] = ('900:F600:18', 4000); sc['credits'] = ('900:F600:1C', 4000)
tot = 0
for name, (poke, n) in sc.items():
    e = dict(os.environ, NOCPU_POKE=poke, PROV_CHECK='1')
    r = subprocess.run([BIN, '--headless', str(n)], env=e, capture_output=True, text=True)
    t = c = k = 0
    for l in r.stderr.splitlines():
        m = re.match(r'origine image \d+ : (\d+) pixels dont (\d+) de sprites ; (\d+) tuiles incohérentes, (\d+) couleurs', l)
        if m: k += 1; t += int(m[3]); c += int(m[4])
    tot += t + c
    print(f'{name:12} {k:3} mesures  tuiles incohérentes {t:6}  couleurs différentes {c:6}', flush=True)
print('TOTAL écarts', tot)
