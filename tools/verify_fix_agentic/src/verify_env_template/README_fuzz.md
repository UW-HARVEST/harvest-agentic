## Bounded fuzz campaigns

Additional files: `build_fuzz.sh` builds the campaign binary; `run_fuzz.sh` runs
it and sets campaign budgets. `docs/` contains vendored official
FuzzTest reference docs (Apache-2.0).

From the translated project directory, after building the Rust cdylib:

```bash
./verify_env/build_fuzz.sh
RUST_LIB_PATH="$(pwd)/target/release/lib<crate>.so" \
  ./verify_env/run_fuzz.sh Suite.Property
```

The runner preserves your working directory and runs one property. Run campaigns
sequentially; each has its own memory budget. Adjust the defaults in the
configuration block at the top of `verify_env/run_fuzz.sh`, or override them
for one invocation:

```bash
FUZZ_DURATION_SECONDS=30 RUST_LIB_PATH="$(pwd)/target/release/lib<crate>.so" \
  ./verify_env/run_fuzz.sh Suite.Property
```

| Setting | Default | Meaning |
|---------|---------|---------|
| `FUZZ_RSS_LIMIT_MB` | `2048` | FuzzTest's per-process RSS soft limit in MiB |
| `FUZZ_HARD_LIMIT_MB` | `3072` | Campaign cgroup memory limit in MiB, with swap disabled |
| `FUZZ_DURATION_SECONDS` | `300` | Time budget for this property |
| `FUZZ_INPUT_TIMEOUT_SECONDS` | `10` | Time limit for each property invocation |

FuzzTest detects an exceeded RSS limit and aborts; it can overshoot before the
check. The cgroup provides a separate hard limit for the campaign and its child
processes. Keep it above the RSS budget so FuzzTest has room to report a failure.
The outer timeout sends TERM 30 seconds after the campaign budget, then KILL
after another 5 seconds if needed. Core dumps are disabled for the campaign.
These limits apply to the campaign, not the agent or its builds. Both build
scripts default to two parallel jobs; set `CMAKE_BUILD_PARALLEL_LEVEL` to change
that. Do not replace their build command with an unbounded `-j`.

The default hard limit requires Linux cgroup v2 with a working systemd user
manager (`systemd-run --user --scope` and memory-controller support). The runner
checks the applied `memory.max` and `memory.swap.max` inside the scope before
starting the binary. If creation or this check fails, it stops without retrying
without the hard limit.
When running inside an already memory-limited container/cgroup without a user
manager, explicitly set `FUZZ_HARD_LIMIT_MB=0` to use that external protection.
This prints a warning and retains the FuzzTest RSS limit and timeouts. Do not
use this override on an unbounded host. Do not substitute `ulimit -v`: it limits
virtual address space and conflicts with ASan shadow mappings.

Record budget increases and their reasons in `HYPOTHESES.md`. Investigate
unexpected growth before raising limits: small inputs can expand into large
outputs, and contexts or retained state can accumulate across iterations.

Each run creates a unique `verify_env/fuzz-artifacts/Suite.Property.*/` directory:

- `run.log`: effective command, budgets, and complete campaign output;
- `exit_status`: launcher/campaign exit code (nonzero means failed/incomplete);
- `reproducers/`: file reproducers via `FUZZTEST_REPRODUCERS_OUT_DIR`;
- `corpus/`: coverage inputs via `FUZZTEST_TESTSUITE_OUT_DIR`.

These artifacts survive removal of `build-*` during snapshot creation. Keep file
reproducers: printed regression drafts can truncate large values. A hard kill
can prevent writing a reproducer, and a cumulative-memory OOM may need more than
the last input to reproduce. OOM, timeout, signal termination, and launcher
failure never count as passing campaigns. If you pipe the runner into another
command, use `set -o pipefail` so the pipeline preserves failures.
