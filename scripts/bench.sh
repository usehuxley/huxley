#!/usr/bin/env bash
# Benchmarks Huxley at a reference size (docs/performance.md) on one
# machine, keeping PostgreSQL, Huxley and the load generator on separate
# cores so they don't compete:
#
#   scripts/bench.sh small|medium|large
#
# Needs Docker, taskset, and 16 hardware threads for `large`. Starts its own
# PostgreSQL container (on 127.0.0.1:5499) and removes it afterwards.
set -euo pipefail
size="${1:-small}"
cd "$(dirname "$0")/.."

# PostgreSQL cores, memory; Huxley's cores; the load generator's cores.
case "$size" in
  small)  db_cpus=0-1;  db_mem=4g;  hx_cpus=2-3;   gen_cpus=8-15;  workers=1; runs=40 ;;
  medium) db_cpus=0-3;  db_mem=8g;  hx_cpus=4-7;   gen_cpus=8-15;  workers=2; runs=80 ;;
  large)  db_cpus=0-5;  db_mem=12g; hx_cpus=6-11;  gen_cpus=12-15; workers=4; runs=160 ;;
  *) echo "usage: $0 small|medium|large" >&2; exit 2 ;;
esac
mem_mb=$(( ${db_mem%g} * 1024 ))
name="huxley-bench-pg"
port=5499

docker rm -f "$name" >/dev/null 2>&1 || true
docker run -d --name "$name" --network host \
  --cpuset-cpus "$db_cpus" --memory "$db_mem" --shm-size 1g \
  -e POSTGRES_USER=huxley -e POSTGRES_PASSWORD=huxley -e POSTGRES_DB=huxley \
  pgvector/pgvector:0.8.2-pg18 \
  -c port=$port -c listen_addresses=127.0.0.1 -c max_connections=400 \
  -c shared_buffers=$(( mem_mb / 4 ))MB -c effective_cache_size=$(( mem_mb * 3 / 4 ))MB \
  -c work_mem=16MB -c maintenance_work_mem=512MB -c synchronous_commit=on >/dev/null
trap 'docker rm -f "$name" >/dev/null 2>&1 || true' EXIT
for _ in $(seq 60); do
  docker exec "$name" pg_isready -h 127.0.0.1 -p $port -U huxley >/dev/null 2>&1 && break
  sleep 1
done

export HUXLEY_TEST_DATABASE_URL="postgres://huxley:huxley@127.0.0.1:$port/huxley"
export BENCH_SIZE="$size" BENCH_HUXLEY_CPUS="$hx_cpus" BENCH_WORKERS="$workers" BENCH_RUNS="$runs"
# Linking every test binary at once can exhaust memory: build them first, a few at a time.
CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" cargo test --release -p huxley-server --test api_bench --test load --no-run -q

echo "== $size: PostgreSQL on CPUs $db_cpus ($db_mem), Huxley on $hx_cpus, load generator on $gen_cpus"
taskset -c "$gen_cpus" cargo test --release -q -p huxley-server --test api_bench -- --ignored --nocapture 2>&1 \
  | grep -E '^(seeded|\||small|medium|large)'
taskset -c "$gen_cpus" cargo test --release -q -p huxley-server --test load -- --ignored --nocapture 2>&1 \
  | grep -E 'steps/s'
