#!/usr/bin/env bash
# Run the unit-test binary (build-test/verification_tests) with two memory
# limits and a time limit. Arguments go to the binary unchanged, for example:
#   RUST_LIB_PATH=<abs path to .so> ./verify_env/run_tests.sh --gtest_filter='Suite.*'
# Preserve the caller's working directory: some differential tests use
# project-relative paths.
set -euo pipefail

# Run defaults: edit here, or override in the environment for one run.
# Record limit increases and their reason in HYPOTHESES.md.
TEST_AS_LIMIT_MB="${TEST_AS_LIMIT_MB:-2048}"       # Per-process address space (ulimit -v), MiB.
TEST_HARD_LIMIT_MB="${TEST_HARD_LIMIT_MB:-3072}"   # cgroup memory for the run, MiB; no swap.
TEST_TIMEOUT_SECONDS="${TEST_TIMEOUT_SECONDS:-600}"
# The hard limit needs systemd-run --user/cgroup v2. Set it explicitly to 0
# only when an external container/cgroup already bounds the run.

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

fail() { echo "run_tests.sh: $*" >&2; exit 2; }
for name in TEST_AS_LIMIT_MB TEST_TIMEOUT_SECONDS; do
  value="${!name}"
  [[ $value =~ ^[1-9][0-9]{0,8}$ ]] || fail "$name must be a positive integer"
done
[[ $TEST_HARD_LIMIT_MB =~ ^(0|[1-9][0-9]{0,8})$ ]] ||
  fail "TEST_HARD_LIMIT_MB must be a nonnegative integer"
if (( TEST_HARD_LIMIT_MB > 0 && TEST_HARD_LIMIT_MB <= TEST_AS_LIMIT_MB )); then
  fail "TEST_HARD_LIMIT_MB must exceed TEST_AS_LIMIT_MB so the per-process limit fails first"
fi
[[ ${RUST_LIB_PATH:-} == /* && -f ${RUST_LIB_PATH:-} ]] ||
  fail "RUST_LIB_PATH must name an existing absolute path to the Rust cdylib"
binary="$here/build-test/verification_tests"
[[ -x $binary ]] || fail "build the test binary with $here/build.sh first"
command -v timeout >/dev/null || fail "required command is missing: timeout"

[[ -r $here/memory_scope.sh ]] || fail "missing $here/memory_scope.sh"
# shellcheck source=memory_scope.sh
source "$here/memory_scope.sh"
memory_scope_launcher "$TEST_HARD_LIMIT_MB" TEST_HARD_LIMIT_MB || exit 2

# The address-space limit is set inside the scope, just before the binary
# starts, so systemd-run itself is not limited. A sanitizer build reserves
# terabytes of shadow address space and cannot run under this limit.
as_limit=(bash -c 'ulimit -v "$1"; shift; exec "$@"' harvest-address-limit
  "$((TEST_AS_LIMIT_MB * 1024))")
if LC_ALL=C grep -q -a -m 1 __asan_init "$binary"; then
  echo "WARNING: $binary uses AddressSanitizer; the address-space limit is skipped, the cgroup limit stays." >&2
  as_limit=()
fi

export RUST_LIB_PATH
# An out-of-memory failure must not create a multi-gigabyte core dump.
ulimit -c 0
set +e
"${MEMORY_SCOPE[@]}" "${as_limit[@]}" timeout --kill-after=5s \
  "${TEST_TIMEOUT_SECONDS}s" "$binary" "$@"
status=$?
set -e
case $status in
  124) echo "run_tests.sh: the run exceeded TEST_TIMEOUT_SECONDS=${TEST_TIMEOUT_SECONDS}." >&2 ;;
  137|143) echo "run_tests.sh: the run was killed (exit $status). The memory cgroup (TEST_HARD_LIMIT_MB=${TEST_HARD_LIMIT_MB}) or the timeout can cause this. Run a smaller --gtest_filter to find the test." >&2 ;;
esac
exit "$status"
