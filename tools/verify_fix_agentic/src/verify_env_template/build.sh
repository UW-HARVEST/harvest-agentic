#!/usr/bin/env bash
# Configure and build the verification test binary in unit-test mode.
# Runs the GoogleTest cases against the C reference and Rust translation.
#
# Requires the translated Rust cdylib to exist (cargo build --release in the
# parent directory) so tests can dlopen it via RUST_LIB_PATH at run time.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$here"

CC="${CC:-clang}" CXX="${CXX:-clang++}" cmake -S . -B build-test \
  -DCMAKE_BUILD_TYPE=RelWithDebInfo
cmake --build build-test --parallel "${CMAKE_BUILD_PARALLEL_LEVEL:-2}"

echo
echo "Built build-test/verification_tests"
echo "Run:  RUST_LIB_PATH=<abs path to translated .so> ./build-test/verification_tests"
