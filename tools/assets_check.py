#!/usr/bin/env python3
"""Met au point le fichier d'assets : relève (espion) les octets de code lus comme données sur tous les scénarios, les conserve
(assets_keep.txt), régénère l'asset et compare les empreintes d'images ROM contre assets, jusqu'à zéro différence."""
import os, subprocess, sys
BIN = './target/release/sonic1_rs'
sc = {"long": ("", 30000)}
for z in range(6):
    for act in (0, 1, 2):
        sc[f"zone{z}-{act}"] = (f"900:F600:0C,900:FE10:{z:02X},900:FE11:{act:02X}", 3500)
sc["bonus"] = ("900:F600:10", 6000); sc["continue"] = ("900:F600:14", 3500); sc["fin"] = ("900:F600:18", 6000)
sc["credits"] = ("900:F600:1C", 6000); sc["demo"] = ("900:F600:08", 4000)
sc["mort"] = ("900:F600:0C,900:FE10:00,900:FE11:00,1700:D024:06", 4000)
def run(poke, n, env=None, assets=None):
    e = dict(os.environ, NOCPU_POKE=poke)
    if env: e.update(env)
    if assets: e['SONIC_ASSETS'] = assets
    r = subprocess.run([BIN, '--nocpu-headless', str(n)], env=e, capture_output=True, text=True, timeout=900)
    return [l for l in (r.stdout + r.stderr).splitlines() if l.startswith('image') or 'panick' in l or 'Error' in l]
for it in range(int(sys.argv[1]) if len(sys.argv) > 1 else 4):
    open('/tmp/trap_all.txt', 'w').close()
    for name, (poke, n) in sc.items():
        run(poke, n, env={'TRAP_CODE': '/tmp/trap_all.txt'})
    keep = set(open('assets_keep.txt').read().split()) | set(open('/tmp/trap_all.txt').read().split())
    open('assets_keep.txt', 'w').write('\n'.join(sorted(keep)) + '\n')
    subprocess.run([BIN, '--make-assets', '/tmp/sonic1.assets'], capture_output=True)
    bad = 0
    for name, (poke, n) in sc.items():
        a, b = run(poke, n), run(poke, n, assets='/tmp/sonic1.assets')
        d = sum(1 for x, y in zip(a, b) if x != y) + abs(len(a) - len(b))
        if d: bad += 1; print(f"  {name}: {d} différences")
    print(f"passe {it}: {len(keep)} octets conservés, {bad} scénarios différents sur {len(sc)}", flush=True)
    if bad == 0: break
