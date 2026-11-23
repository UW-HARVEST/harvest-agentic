# Verification Hypotheses Log

The framework wrote this file for the main agent of the verification. Keep the
`Invariants` section as it is. The hypothesis log is append-only: add an entry
for each bug hypothesis, and update its status.

## Invariants (do not drift across compactions)

These rules govern verification. They must survive every compaction unchanged.

{MODEL_LIMITS}

{RUST_TOOLCHAIN_CONTEXT}

{CASE_LAYOUT}

### AFTER ANY COMPACTION: read `PLAN.md` and `HYPOTHESES.md` is your first action before anything.

### Ground truth
- The C code is the authoritative reference. Rust outputs must match C
  byte-for-byte on the whole public surface: every library call result, and
  every executable run result (stdout, stderr, exit status, written files).
- If C and Rust diverge, fix Rust. NEVER modify C.

{CARGO_FEATURES}

### Cargo manifest target names
- `[lib] name` and `[[bin]] name` MUST use underscores only — NO hyphens.
  Hyphens cause manifest parse failure. Fix Cargo.toml if you see them.

### Boundaries
- Do NOT modify anything in `inputs/c_src`.

### Configuration coverage
- Test every configuration (see the Cargo features invariant): the grader
  can test any of them. When there are too many combinations, test every
  value of each option at least once and record the choice in `CONFIGS.md`.
  For each configuration you test:
    1. Clean and rebuild C with the option values as cmake `-D` flags.
    2. Rebuild Rust with its Cargo features.
    3. Re-run integration tests and fix any mismatches before moving on.

### Surface coverage and completion
- `SYMBOLS.md`, `CONDITIONALS.md`, `ERRORS.md`, and `CONFIGS.md` are the test
  plan. Every `CONFIGS.md` row gets a differential test. Every `ERRORS.md`
  row gets a rejection test that asserts both sides return the same error
  code, sentinel value, or exit status and message.
- Every region in `CONDITIONALS.md` is settled: conditional compilation in
  Rust, or a resolution to one branch with recorded evidence.
- An untested row or an inactive-branch judgment carries evidence: an
  experiment, a definition-site grep, or a premise in `CONDITIONALS.md`.
  Reading alone is not evidence.
- Verification is complete only when `SYMBOLS.md` has no open missing entry
  (for the library, the `nm -D` symbol diff is empty), every row has a
  passing test under every configuration you test, every
  `CONDITIONALS.md` region is settled, and the final cross-review finds no
  gap. Green tests on inputs you picked yourself are not completion.

### Git discipline
{GIT_DISCIPLINE}

### Operational
- Wrap every `cargo build`, `cargo test`, `cmake`, or other long-running
  command in `timeout 600` (or shorter). No single command should run > 600s.
- If a single test takes too long, skip it and move on. Do not get stuck on
  one step.
{HARNESS_INVARIANT}
## Hypothesis log

Format per entry:
### H<N>: <one-line hypothesis>
- Status: open | confirmed | refuted | fixed
- Evidence: <how I think I know>
- Coverage: <ERRORS.md #N / CONFIGS.md #M / CONDITIONALS.md #K / test name; omit when not tied to a row or a test>
- Files/lines suspected: <file path:line>
- Action taken: <Edit/test/none yet>
- Outcome: <what happened after the action>
