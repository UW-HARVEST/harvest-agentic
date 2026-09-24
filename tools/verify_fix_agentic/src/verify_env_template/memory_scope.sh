# Memory cgroup launcher shared by the verify_env run scripts.
# Source this file from a run script. Do not run it directly.
#
# memory_scope_launcher LIMIT_MB SWITCH_NAME
#   Sets the array MEMORY_SCOPE to a command prefix. The prefix starts the
#   rest of the command in a systemd user scope with memory.max = LIMIT_MB MiB
#   and no swap. Inside the scope it checks that the kernel applied both
#   limits before it starts the command.
#   LIMIT_MB=0 leaves MEMORY_SCOPE empty and prints a warning. Use 0 only when
#   an external container or cgroup already bounds the run.
#   SWITCH_NAME is the setting that holds LIMIT_MB, for the messages.
#   Returns 2 when systemd-run is unavailable.
memory_scope_launcher() {
  local limit_mb="$1" switch_name="$2"
  MEMORY_SCOPE=()
  if (( limit_mb == 0 )); then
    echo "WARNING: $switch_name=0 disables the memory cgroup; external memory protection is required." >&2
    return 0
  fi
  if ! command -v systemd-run >/dev/null; then
    echo "${0##*/}: systemd-run is unavailable; use an external cgroup/container and explicitly set $switch_name=0 (see README.md)" >&2
    return 2
  fi
  # Failure to create the scope stops the run; never retry the binary unbounded.
  # A user manager can exist on systems without a delegated memory controller;
  # accepting a property alone is not proof that the run is bounded.
  # shellcheck disable=SC2016 # Expand these variables inside the scoped shell.
  MEMORY_SCOPE=(systemd-run --user --scope --quiet
    -p "MemoryMax=${limit_mb}M" -p MemorySwapMax=0
    bash -c '
    set -euo pipefail
    expected="$1"; shift
    cgroup=""
    while IFS=: read -r hierarchy controllers path; do
      if [[ $hierarchy == 0 && -z $controllers ]]; then cgroup="$path"; break; fi
    done < /proc/self/cgroup
    base="/sys/fs/cgroup$cgroup"
    if [[ -z $cgroup || ! -r $base/memory.max || ! -r $base/memory.swap.max ]]; then
      echo "Run stopped: cgroup v2 memory controls are unavailable." >&2
      exit 2
    fi
    if [[ $(< "$base/memory.max") != "$((expected * 1024 * 1024))" ||
          $(< "$base/memory.swap.max") != 0 ]]; then
      echo "Run stopped: requested cgroup memory/swap limits were not applied." >&2
      exit 2
    fi
    exec "$@"
  ' harvest-memory-check "$limit_mb")
}
