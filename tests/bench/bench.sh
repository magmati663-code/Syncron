#!/usr/bin/env bash
# Benchmark Syncron CLI: skan + pierwszy przebieg + pusty przebieg + zmiana jednego pliku.
# Użycie: tests/bench/bench.sh [N_PLIKOW] [SCIEZKA_CLI]
set -euo pipefail
N=${1:-20000}
CLI=${2:-/home/user/target-shared/release/syncron}
W=$(mktemp -d /tmp/syncron-bench.XXXXXX)
trap 'rm -rf "$W"' EXIT
mkdir -p "$W/l" "$W/r" "$W/data"
python3 - "$W/l" "$N" <<'P'
import os,sys
root,n=sys.argv[1],int(sys.argv[2])
for i in range(n):
    d=os.path.join(root,f"d{i%200:03d}",f"s{i%7}")
    os.makedirs(d,exist_ok=True)
    with open(os.path.join(d,f"plik_{i}_zażółć.txt"),"w") as f: f.write(f"dane {i}\n"*(1+i%20))
P
cat > "$W/job.json" <<J
[{"id":"bench","name":"bench","left":"$W/l","right":"$W/r","mode":"TwoWay","conflict":"Newer","schedule":{"type":"Manual"}}]
J
S="$CLI --data-dir $W/data"
$S import "$W/job.json" >/dev/null
t() {
  python3 - "$LABEL" "$@" <<'P'
import resource,subprocess,sys,time
label,cmd=sys.argv[1],sys.argv[2:]
s=time.time(); subprocess.run(cmd,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL); e=time.time()
print(f"{label:<28} {e-s:6.2f} s   RSS {resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss//1024:6d} MB")
P
}
echo "Pliki: $N"
LABEL="pierwszy przebieg (kopia)" t $S run bench
LABEL="pusty przebieg" t $S run bench
echo "zmiana" >> "$(find "$W/l" -name 'plik_5_*' | head -1)"
LABEL="jedna zmiana" t $S run bench
LABEL="symulacja (dry-run)" t $S run bench --dry-run
