#!/usr/bin/env bash
# Configure and build the verification test binary in unit-test mode.
# Runs the GoogleTest cases against the C reference and Rust translation.
#
# Requires the translated Rust cdylib to exist (cargo build --release in the
# parent directory) so tests can dlopen it via RUST_LIB_PATH at run time.
# Run the tests with run_tests.sh, which applies memory and time limits.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=shared_build.sh
source "$here/shared_build.sh"

CC="${CC:-clang}" CXX="${CXX:-clang++}" shared_build build-test \
  -DCMAKE_BUILD_TYPE=RelWithDebInfo -DHARVEST_C_OPTIONS="${HARVEST_C_OPTIONS:-}"

echo
echo "Built build-test/verification_tests"
echo "Run:  RUST_LIB_PATH=<abs path to translated .so> $here/run_tests.sh [GoogleTest flags]"
echo "Limits: see the configuration block at the top of run_tests.sh"
