#!/bin/bash
# Lance data_diff.sh sur de nombreux scénarios (zones, actes, bonus, fin, mort, coup reçu...). Usage : tools/data_scenarios.sh ASSETS [ROM] [IMAGES]
A="$1"; R="${2:-/home/kevin/Projet_IA/Jeux_video/binaries_source/Sonic The Hedgehog (USA, Europe).md}"; N="${3:-4500}"
run() { tools/data_diff.sh "$A" "$R" "$N" "$2" "$1"; }
for z in 0 1 2 3 4 5; do for act in 0 1 2; do
  [ $z = 5 ] && [ $act != 0 ] && continue
  run "zone$z-acte$act" "900:F600:0C,900:FE10:0$z,900:FE11:0$act" &
  [ $(jobs -r | wc -l) -ge 4 ] && wait -n
done; done
run bonus "900:F600:10" &
run continue "900:F600:14" &
wait -n
run fin "900:F600:18" &
run credits "900:F600:1C" &
wait -n
run demo "900:F600:08" &
run mort "900:F600:0C,900:FE10:00,900:FE11:00,1700:D024:06" &
wait -n
run coup-motobug "900:F600:0C,900:FE10:00,900:FE11:00,1575:E080:40,1575:E088:07,1575:E089:90,1575:E08C:03,1575:E08D:72,1575:FE21:20" &
run piques "900:F600:0C,900:FE10:00,900:FE11:00,1500:E080:36,1500:E088:05,1500:E089:55,1500:E08C:03,1500:E08D:78,1500:FE21:30" &
wait
