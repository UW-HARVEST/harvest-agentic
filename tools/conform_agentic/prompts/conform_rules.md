# Rules of the conformance stage

Reader: the main agent of the conformance stage, and each sub-agent whose task message names this
file.

## Ground truth
- The EXTERNAL test suite in `inputs/suite/` defines success. The goal is
  zero external-test failures.
- `inputs/c_src` is the semantic reference explaining WHY a test expects a value.
  If Rust and C disagree, the C behavior is correct — fix Rust.

## Boundaries
- Only `src/` and `Cargo.toml` may change. NEVER modify `inputs/c_src`, the external
  test suite (`inputs/suite/`), or `tests/`.
- The final grade re-runs the SAME external suite from a pristine copy, so any
  edit to the tests is discarded and counts as cheating. Editing tests can
  never help you.

## No overfitting
- Fix the underlying algorithm, never the specific test input. Do NOT
  special-case a test's exact bytes, hardcode an expected output, or branch on
  a recognized input. A fix that only works for the inputs you can see is a
  failed fix.

## Full public ABI
- Every non-`static` function the C shared library exports must also be
  exported by the Rust crate (verified with `nm -D`); a missing symbol makes
  the suite fail to link. Add any that are missing.
