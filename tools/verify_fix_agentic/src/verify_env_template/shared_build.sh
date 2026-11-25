# Shared build of the test binary, for the build scripts of verify_env/.
# Source this file from a build script. Do not run it directly.
#
# shared_build NAME [CMake configure arguments...]
#   Builds verification_tests from the test files of this verify_env/
#   (verification_tests.cc and tests/*.cc), and copies it to
#   <this verify_env>/NAME/verification_tests.
#   All checkouts of the workspace (the main checkout and its git worktrees)
#   build in one directory, <main checkout>/verify_env/NAME-shared, with the
#   CMakeLists.txt of the main checkout, so the test frameworks are compiled
#   only once. A lock lets one build run at a time. Ninja, when present, keeps
#   the objects of each checkout across builds of other checkouts. Without
#   git 2.31 or a git checkout, the build directory is this verify_env's own.
shared_build() {
  local name="$1"; shift
  local main_env="$here" common
  if common="$(git -C "$here" rev-parse --path-format=absolute --git-common-dir 2>/dev/null)" &&
     [[ -f "$(dirname "$common")/verify_env/CMakeLists.txt" ]]; then
    main_env="$(dirname "$common")/verify_env"
  fi
  if ! cmp -s "$here/CMakeLists.txt" "$main_env/CMakeLists.txt"; then
    echo "WARNING: this build uses $main_env/CMakeLists.txt; the changes to $here/CMakeLists.txt are not used." >&2
  fi
  local shared="$main_env/$name-shared" generator=()
  if [[ ! -f $shared/CMakeCache.txt ]] && command -v ninja >/dev/null; then
    generator=(-G Ninja)
  fi
  mkdir -p "$shared" "$here/$name"
  (
    if ! flock -n 9; then
      echo "Waiting for the build of another checkout to end ..." >&2
      flock 9
    fi
    cmake -S "$main_env" -B "$shared" "${generator[@]}" -DHARVEST_TEST_DIR="$here" "$@"
    cmake --build "$shared" --parallel "${CMAKE_BUILD_PARALLEL_LEVEL:-2}"
    cp "$shared/verification_tests" "$here/$name/verification_tests.new"
    mv "$here/$name/verification_tests.new" "$here/$name/verification_tests"
  ) 9>"$shared/.lock"
}
