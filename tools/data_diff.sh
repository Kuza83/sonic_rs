#!/bin/bash
# Compare, image par image, l'exécution avec les données de la ROM complète et avec le fichier d'assets (même code recompilé).
# (on compare les images affichées ; la RAM diffère de toute façon, sans effet visible.) Toute différence = des octets de données effacés à tort dans les assets. Usage : tools/data_diff.sh ASSETS ROM IMAGES "POKES" [nom]
A="$1"; R="$2"; N="$3"; export NOCPU_POKE="$4"; export HASH_EVERY=1
BIN=${BIN:-./sonic1}
SONIC_ASSETS="$R" $BIN --headless "$N" 2>&1 | grep '^H ' | awk '{print $2,$3}' > /tmp/dd_rom_$$.txt &
SONIC_ASSETS="$A" $BIN --headless "$N" 2>&1 | grep '^H ' | awk '{print $2,$3}' > /tmp/dd_ast_$$.txt
wait
first=$(diff /tmp/dd_rom_$$.txt /tmp/dd_ast_$$.txt | grep -m1 '^>' | awk "{print \$2}")
if [ -n "$first" ]; then echo "${5:-scenario}: PREMIERE DIFFERENCE a l image $first"; else echo "${5:-scenario}: identique sur $N images"; fi
rm -f /tmp/dd_rom_$$.txt /tmp/dd_ast_$$.txt
