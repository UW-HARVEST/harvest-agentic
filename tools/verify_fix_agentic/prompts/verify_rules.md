# Rules of the verification

The sections "Ground truth", "Surface rows", "Generic boundaries", "Evidence" and "Operational"
apply to every reader. The section "Duties of the main agent" is for the main agent only. A sub-agent does only
the task that its task message gives.

## Ground truth
- The C code is the authoritative reference. Rust outputs must match C
  byte-for-byte on the whole public surface: every library call result, and
  every executable run result (stdout, stderr, exit status, written files).
- If C and Rust diverge, fix Rust. NEVER modify C.

## Surface rows
- `SYMBOLS.md`, `CONDITIONALS.md`, `ERRORS.md`, and `CONFIGS.md` are the test
  plan. Every `CONFIGS.md` row gets a differential test on valid inputs. Every
  `ERRORS.md` row gets a rejection test that asserts both sides return the same
  error code, sentinel value, or exit status and message, not only that both
  sides fail.
- A row that cannot be tested stays in its file, unchecked, with a note that
  carries evidence (see "Evidence"). Do not remove a row, and do not move it to
  another file. Remove a row only when it is wrong (for example, it names an
  entry point that does not exist), with the evidence in `HYPOTHESES.md`.

## Generic boundaries
The tests of the verification cover the generic boundaries, also when no row of the surface files
lists them: null pointers, zero and oversized lengths, values one step past a valid range, and enum
arguments with no valid variant. The C code takes any int where an API declares an enum, and answers such calls through
its default or error branch. Rust must answer the same way. For the executable, the generic
boundaries are no arguments, unknown options, missing and extra option values, empty stdin, and
input without a final newline.

## Evidence
- An untested row or an inactive-branch judgment carries evidence: an
  experiment, a definition-site grep, or a premise in `CONDITIONALS.md`.
  Reading alone is not evidence.

## Duties of the main agent

### Configuration coverage
- Test every configuration (see `inputs/prompts/cargo_features.md`): the grader
  can test any of them. When there are too many combinations, test every
  value of each option at least once and record the choice in `CONFIGS.md`.
  For each configuration you test:
    1. Clean and rebuild C with the option values as cmake `-D` flags.
    2. Rebuild Rust with its Cargo features.
    3. Re-run integration tests and fix any mismatches before moving on.

### Completion
- Every region in `CONDITIONALS.md` is settled: conditional compilation in
  Rust, or a resolution to one branch with recorded evidence.
- Verification is complete only when `SYMBOLS.md` has no open missing entry
  (for the library, the `nm -D` symbol diff is empty), every row has a
  passing test under every configuration you test, every
  `CONDITIONALS.md` region is settled, and the review rule of the completion
  gate in `inputs/prompts/verify/main.md` is met. Green tests on inputs you
  picked yourself are not completion.

## Operational
- The time limit of "Long commands" in `inputs/prompts/common.md` applies to
  every `cargo build`, `cargo test` and `cmake` command.
- If a single test takes too long, skip it and move on. Do not get stuck on
  one step.
