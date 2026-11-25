#!/usr/bin/env bash
# Configure and build the verification test binary in fuzzing mode: Clang with
# coverage instrumentation and sanitizers, for a coverage-guided FUZZ_TEST
# campaign. Uses a separate build directory (build-fuzz) — never toggle the
# fuzzing flag inside an existing build dir.
#
# Requires the translated Rust cdylib to exist. Then run a campaign with:
#   RUST_LIB_PATH=<abs path to .so> \
#     ./run_fuzz.sh Suite.PropertyName
# Campaign budgets are configured at the top of run_fuzz.sh.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=shared_build.sh
source "$here/shared_build.sh"

CC=clang CXX=clang++ shared_build build-fuzz \
  -DCMAKE_BUILD_TYPE=RelWithDebInfo \
  -DFUZZTEST_FUZZING_MODE=ON

echo
echo "Built build-fuzz/verification_tests"
echo "Fuzz: RUST_LIB_PATH=<abs path to translated .so> \\"
echo "      ./run_fuzz.sh Suite.PropertyName"
echo "Budgets: edit the configuration block at the top of run_fuzz.sh"
