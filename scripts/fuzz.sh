#!/bin/sh
# Runs every libFuzzer target for N seconds (default 60) in parallel.
# Needs cargo-fuzz and a nightly toolchain via rustup. Usage: scripts/fuzz.sh [seconds]
set -e
cd "$(dirname "$0")/../fuzz"
export PATH="$HOME/.cargo/bin:$PATH" RUSTUP_TOOLCHAIN=nightly
secs="${1:-60}"
cargo fuzz build
targets=$(cargo fuzz list)
for t in $targets; do
    mkdir -p "corpus/$t"
    cargo fuzz run "$t" -- -max_total_time="$secs" -rss_limit_mb=2048 -max_len=300000 > "artifacts-$t.log" 2>&1 &
done
wait
status=0
for t in $targets; do
    if grep -qE "panicked at|ERROR: libFuzzer" "artifacts-$t.log"; then
        echo "FAIL $t (see fuzz/artifacts-$t.log and fuzz/artifacts/$t)"; status=1
    else
        echo "ok   $t: $(grep -E '^#[0-9]+' "artifacts-$t.log" | tail -1 | cut -c1-60)"
    fi
done
rm -f artifacts-*.log
exit $status
