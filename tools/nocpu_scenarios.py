#!/usr/bin/env python3
"""Lance la découverte du moteur sans CPU sur plusieurs scénarios (modes de jeu et zones forcés par écriture en RAM)."""
import os, subprocess, sys
sc = {}
for z in range(6):
    for act in (0, 1):
        sc[f"zone{z}-act{act}"] = f"900:F600:0C,900:FE10:{z:02X},900:FE11:{act:02X}"
sc["bonus"] = "900:F600:10"
sc["continue"] = "900:F600:14"
sc["fin"] = "900:F600:18"
sc["credits"] = "900:F600:1C"
sc["demo"] = "900:F600:08"
sc["mort"] = "900:F600:0C,900:FE10:00,900:FE11:00,1700:D024:06"
sc["mort-vies0"] = "900:F600:0C,900:FE10:02,900:FE11:00,900:FE12:01,1700:D024:06"
only = sys.argv[1:]
for name, poke in sc.items():
    if only and name not in only: continue
    env = dict(os.environ, NOCPU_POKE=poke)
    print('==', name, flush=True)
    r = subprocess.run(['python3', 'nocpu_iterate.py', '3500', '30'], env=env, capture_output=True, text=True)
    print(r.stdout.strip().splitlines()[-1] if r.stdout.strip() else r.stderr[-300:], flush=True)
