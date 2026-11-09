#!/bin/sh
# Une passe de découverte : joue la partie de test, relève le code encore interprété, l'ajoute comme points d'entrée, régénère.
cd "$(dirname "$0")"
rm -f /tmp/p_z.txt /tmp/p_o.txt
PROFILE_OUT=/tmp/p_z.txt SONIC_PROFILE=1 SONIC_NOCHECK=1 VERIFY_ZONES=1 timeout 200 ./target/release/sonic1_rs --verify 3800 > /dev/null 2>&1
PROFILE_OUT=/tmp/p_o.txt SONIC_PROFILE=1 SONIC_NOCHECK=1 VERIFY_OBJECTS=1 timeout 200 ./target/release/sonic1_rs --verify 3800 > /dev/null 2>&1
python3 - <<'PY'
addrs = {}
for f in ['/tmp/p_z.txt', '/tmp/p_o.txt']:
    for l in open(f):
        a, n = l.split(); addrs[int(a, 16)] = addrs.get(int(a, 16), 0) + int(n)
ks = sorted(addrs); rng = []; s = p = ks[0]
for a in ks[1:]:
    if a - p > 8: rng.append((s, p)); s = a
    p = a
rng.append((s, p))
old = set(int(x, 16) for x in open('seeds_extra.txt').read().split(',') if x)
new = {a for a, b in rng if a >= 0x200 and a % 2 == 0} - old
open('seeds_extra.txt', 'w').write(','.join('%x' % a for a in sorted(old | new)))
print(f"{len(addrs)} adresses encore interprétées, {len(rng)} plages, {len(new)} nouveaux points d'entrée")
PY
./regen.sh
