#!/usr/bin/env python3
"""Découverte pour le moteur sans CPU : relance le jeu jusqu'à ce qu'il tombe sur du code non traduit,
ajoute cette adresse aux points d'entrée (seeds_extra.txt), régénère, recommence."""
import re, subprocess, sys
frames = sys.argv[1] if len(sys.argv) > 1 else '30000'
for it in range(int(sys.argv[2]) if len(sys.argv) > 2 else 12):
    r = subprocess.run(['./target/release/sonic1_rs', '--nocpu-headless', frames], capture_output=True, text=True, timeout=600)
    out = r.stdout + r.stderr
    m = re.search(r"termin[ée]+ en (0x[0-9a-f]+) au lieu", out) or re.search(r"sortie du code généré en (0x[0-9a-f]+)", out) \
        or re.search(r"gestionnaire d'interruption (0x[0-9a-f]+) non traduit", out)
    last = re.findall(r'image (\d+) :', out)
    print(f"passe {it}: dernière image {last[-1] if last else '?'}; ", end='')
    if not m:
        print('pas de code non traduit rencontré' if r.returncode == 0 else 'échec:\n' + out[-600:])
        break
    a = int(m.group(1), 16)
    print(f'code non traduit en {a:#x}')
    seeds = set(x for x in open('seeds_extra.txt').read().split(',') if x)
    seeds.add('%x' % a)
    open('seeds_extra.txt', 'w').write(','.join(sorted(seeds)))
    subprocess.run(['./regen.sh'], capture_output=True)
