#!/usr/bin/env bash
# Run one property with the budgets below. Preserve the caller's
# working directory: some differential tests use project-relative paths.
set -euo pipefail

# Campaign defaults: edit here, or override in the environment for one run.
# Record budget increases and their reason in HYPOTHESES.md.
FUZZ_RSS_LIMIT_MB="${FUZZ_RSS_LIMIT_MB:-2048}"       # Per-process soft RSS, MiB.
FUZZ_HARD_LIMIT_MB="${FUZZ_HARD_LIMIT_MB:-3072}"     # cgroup memory, MiB; no swap.
FUZZ_DURATION_SECONDS="${FUZZ_DURATION_SECONDS:-300}"
FUZZ_INPUT_TIMEOUT_SECONDS="${FUZZ_INPUT_TIMEOUT_SECONDS:-10}"
# The hard limit needs systemd-run --user/cgroup v2. Set it explicitly to 0
# only when an external container/cgroup already bounds the campaign.

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

fail() { echo "run_fuzz.sh: $*" >&2; exit 2; }
if [[ $# != 1 || ! $1 =~ ^[[:alnum:]_]+\.[[:alnum:]_]+$ ]]; then
  fail "usage: $0 Suite.Property (edit campaign defaults at the top of $0)"
fi
property="$1"
for name in FUZZ_RSS_LIMIT_MB FUZZ_DURATION_SECONDS FUZZ_INPUT_TIMEOUT_SECONDS; do
  value="${!name}"
  [[ $value =~ ^[1-9][0-9]{0,8}$ ]] || fail "$name must be a positive integer"
done
[[ $FUZZ_HARD_LIMIT_MB =~ ^(0|[1-9][0-9]{0,8})$ ]] ||
  fail "FUZZ_HARD_LIMIT_MB must be a nonnegative integer"
if (( FUZZ_HARD_LIMIT_MB > 0 && FUZZ_HARD_LIMIT_MB <= FUZZ_RSS_LIMIT_MB )); then
  fail "FUZZ_HARD_LIMIT_MB must exceed FUZZ_RSS_LIMIT_MB to allow reporting headroom"
fi
[[ ${RUST_LIB_PATH:-} == /* && -f ${RUST_LIB_PATH:-} ]] ||
  fail "RUST_LIB_PATH must name an existing absolute path to the Rust cdylib"
binary="$here/build-fuzz/verification_tests"
[[ -x $binary ]] || fail "build the fuzzing binary with $here/build_fuzz.sh first"
for tool in timeout tee mktemp; do
  command -v "$tool" >/dev/null || fail "required command is missing: $tool"
done

launcher=()
memory_check=()
if (( FUZZ_HARD_LIMIT_MB > 0 )); then
  command -v systemd-run >/dev/null ||
    fail "systemd-run is unavailable; use an external cgroup/container and explicitly set FUZZ_HARD_LIMIT_MB=0 (see README.md)"
  # Failure to create the scope stops the run; never retry the binary unbounded.
  launcher=(systemd-run --user --scope --quiet
    -p "MemoryMax=${FUZZ_HARD_LIMIT_MB}M" -p MemorySwapMax=0)
  # Verify kernel enforcement inside the scope. A user manager can exist on
  # systems without a delegated memory controller; accepting a property alone
  # is not proof that the campaign is bounded.
  # shellcheck disable=SC2016 # Expand these variables inside the scoped shell.
  memory_check=(bash -c '
    set -euo pipefail
    expected="$1"; shift
    cgroup=""
    while IFS=: read -r hierarchy controllers path; do
      if [[ $hierarchy == 0 && -z $controllers ]]; then cgroup="$path"; break; fi
    done < /proc/self/cgroup
    base="/sys/fs/cgroup$cgroup"
    if [[ -z $cgroup || ! -r $base/memory.max || ! -r $base/memory.swap.max ]]; then
      echo "Campaign stopped: cgroup v2 memory controls are unavailable." >&2
      exit 2
    fi
    if [[ $(< "$base/memory.max") != "$((expected * 1024 * 1024))" ||
          $(< "$base/memory.swap.max") != 0 ]]; then
      echo "Campaign stopped: requested cgroup memory/swap limits were not applied." >&2
      exit 2
    fi
    exec "$@"
  ' harvest-fuzz-memory-check "$FUZZ_HARD_LIMIT_MB")
else
  echo "WARNING: runner cgroup disabled explicitly; external memory protection is required." >&2
fi

mkdir -p "$here/fuzz-artifacts"
artifacts="$(mktemp -d "$here/fuzz-artifacts/${property}.XXXXXX")"
mkdir -p "$artifacts/reproducers" "$artifacts/corpus"
export RUST_LIB_PATH
export FUZZTEST_REPRODUCERS_OUT_DIR="$artifacts/reproducers"
export FUZZTEST_TESTSUITE_OUT_DIR="$artifacts/corpus"
# An OOM report must not create a multi-gigabyte core dump.
ulimit -c 0
command=("${launcher[@]}" "${memory_check[@]}" timeout --kill-after=5s
  "$((FUZZ_DURATION_SECONDS + 30))s" "$binary"
  "--fuzz=$property" "--fuzz_for=${FUZZ_DURATION_SECONDS}s"
  "--rss_limit_mb=$FUZZ_RSS_LIMIT_MB"
  "--time_limit_per_input=${FUZZ_INPUT_TIMEOUT_SECONDS}s")
{
  echo "Budgets: RSS=${FUZZ_RSS_LIMIT_MB}MiB, cgroup=${FUZZ_HARD_LIMIT_MB}MiB, duration=${FUZZ_DURATION_SECONDS}s, input=${FUZZ_INPUT_TIMEOUT_SECONDS}s"
  echo "Adjust defaults at the top of: $here/run_fuzz.sh"
  echo "Working directory: $PWD"
  echo "Rust library: $RUST_LIB_PATH"
  echo "Artifacts: $artifacts"
  # Keep the recorded invocation readable; the memory guard above is fixed
  # runner code, not a user-supplied campaign option.
  printf 'Campaign: '; printf '%q ' "$binary" "${command[@]: -4}"; printf '\n'
} | tee "$artifacts/run.log"

set +e
"${command[@]}" 2>&1 | tee -a "$artifacts/run.log"
statuses=("${PIPESTATUS[@]}")
set -e
status="${statuses[0]}"
# A failed log write must not be reported as a successful campaign either.
if (( status == 0 && statuses[1] != 0 )); then status="${statuses[1]}"; fi
printf '%s\n' "$status" > "$artifacts/exit_status"
if (( status != 0 )); then
  echo "Campaign failed or incomplete (exit $status). Inspect $artifacts/run.log; do not count this as a pass." >&2
fi
exit "$status"
