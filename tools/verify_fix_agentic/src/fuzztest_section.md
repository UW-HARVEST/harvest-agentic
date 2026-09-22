### Property-based fuzzing with FuzzTest

`verify_env/` is also set up for FuzzTest,
a property-based, coverage-guided fuzzing layer that runs on top of GoogleTest.
It is an **additional** capability, but do not treat it as exotic. A handful of
fixed `TEST` cases only pins down the exact inputs you happened to pick, and
hand-chosen inputs systematically miss the regions where C and Rust diverge —
so wherever an API has an input *dimension* worth exploring, the default should
be a `FUZZ_TEST` over that dimension (alongside any fixed regression tests),
not a few static values.

Concretely, add a `FUZZ_TEST` covering each of these dimensions when the API
under test has them:

- **The primary input payload** — the main thing the API consumes, usually
  something like a byte stream, a string, an input program/document, or the raw
  numeric operands of a computation. Fuzz it as the corresponding value
  (`std::vector<uint8_t>`, `std::string`, a typed operand, ...); do not settle
  for a few hand-written literals.
- **Any scalar/string/enum parameter that changes behavior** — a level,
  mode/flag, size/count, offset, or path selector that steers the function down
  different code paths. Give the parameter a domain (below) so the fuzzer varies it and
  the coverage guidance can drive each branch.

Pick the mechanism per area of the API. Broad valid-path areas belong in
`FUZZ_TEST` properties, because the fuzzer explores them far beyond what
hand-picked inputs reach. An error path also belongs in a property when its
triggers are cheap to express as a domain: derive invalid sizes, buffer
offsets, or enum values from fuzz input bytes and let the fuzzer sweep them
against the rejection contract. When a trigger is too specific for a domain,
write a fixed single-point `TEST` for that `ERRORS.md` row instead. Both
mechanisms count as coverage of the row; pick whichever expresses the row
with less harness code.

Choose a domain that covers the legal range (see the domain
guide below and, for anything not covered here, `verify_env/docs/`). Refer to the C documentation and implementation to determine the legal range of each parameter.

A property is an ordinary C++ function that asserts an invariant; you register it
with `FUZZ_TEST` and describe the legal inputs with domains. The same file can
hold both plain `TEST`s and `FUZZ_TEST`s:

```cpp
#include "fuzztest/fuzztest.h"
#include "gtest/gtest.h"

// Differential property: C and Rust must agree on every generated input.
void CompressEquivalent(int level, const std::vector<uint8_t>& input) {
  EXPECT_EQ(RunC(level, input), RunRust(level, input));
}
FUZZ_TEST(CompressDifferential, CompressEquivalent)
    .WithDomains(fuzztest::InRange(1, 12),
                 fuzztest::VectorOf(fuzztest::Arbitrary<uint8_t>())
                     .WithMaxSize(64 * 1024));
```

Choosing domains — describe *what inputs are legal*, do not filter after the fact:

- Integers: `fuzztest::InRange(lo, hi)`; `fuzztest::Arbitrary<int>()` for the full range.
- Enums: `fuzztest::ElementOf<E>({E::kA, E::kB})` (never fuzz a raw int then cast).
- Buffers: fuzz a `std::vector<uint8_t>` and pass `.data()`/`.size()` — never fuzz
  a pointer and a length as independent parameters. Cap size with `.WithMaxSize(n)`
  so a single input does not get too slow.
- Strings: `fuzztest::Arbitrary<std::string>()`, `fuzztest::Utf8String()`, or
  `fuzztest::InRegexp("...")` for simple formats.
- Dependent parameters (e.g. level range depends on algorithm): wrap them in a
  struct and build it with `fuzztest::StructOf` / `fuzztest::FlatMap`.
- Avoid `fuzztest::Filter` over a low-acceptance predicate — most inputs get
  discarded and the campaign wastes its budget. Generate structured values directly.

The above is a cheat-sheet, not the full story. The complete official FuzzTest
reference is vendored under `verify_env/docs/` for you to read on demand — go
there when you need a domain or macro this section does not cover:
`domains-reference.md` (every domain and combinator, with examples),
`fuzz-test-macro.md` (`FUZZ_TEST`, `.WithDomains`, `.WithSeeds`),
`flags-reference.md` (runtime flags), `use-cases.md` (differential-testing
patterns).

Two build modes (use separate build directories — do not toggle the flag in place):

- **Unit-test mode** (default build): plain `TEST`s run normally and each
  `FUZZ_TEST` runs briefly as a smoke check. Good for a fast compile-and-check.
- **Fuzzing mode**: an instrumented, coverage-guided campaign against one property.
  `verify_env/build_fuzz.sh` configures it (Clang + `-DFUZZTEST_FUZZING_MODE=ON`).
  Run a campaign with:

  ```
  RUST_LIB_PATH=<abs path to the Rust .so> \
    ./verify_env/run_fuzz.sh CompressDifferential.CompressEquivalent
  ```

  Run this from the project directory. `verify_env/README.md` describes budgets,
  environment overrides, system prerequisites, and saved artifacts.

Running discipline — a `FUZZ_TEST` is not exercised until you run a real
campaign:

- Run fuzz campaigns through `verify_env/run_fuzz.sh Suite.Property`, one at a
  time. Adjust budgets in the configuration block at the top of that script
  or through its environment variables; record effective budgets, changes,
  and reasons in HYPOTHESES.md. Do not bypass resource limits to make an OOM disappear.
- OOM, timeout, signal termination, or a failed launcher is an incomplete
  campaign. Keep the log and file reproducer under
  `verify_env/fuzz-artifacts/`. See `verify_env/README.md` for execution details.
- Running the ordinary test binary (unit-test mode) only samples each
  `FUZZ_TEST` for about a second with no coverage feedback — a smoke check, not
  fuzzing. It will catch shallow divergences but never the ones that need a
  structured or rare-shaped input. Do not treat "the test binary passed" as
  "the property was fuzzed".
- For each property worth fuzzing, run at least one fuzzing-mode campaign
  (`build_fuzz.sh`, then `run_fuzz.sh Suite.Prop`).
  Confirm it is a real campaign: the output must show `Corpus size` / `Edges covered` /
  `Total runs` climbing. If edges stay flat at the first value, the campaign is
  not making progress.
- Let it run until coverage stops climbing meaningfully (edges plateau), not
  just a token few seconds. A longer `FUZZ_DURATION_SECONDS` on the property covering the
  most behavior is worth more than many one-second runs.
- When a campaign finds a mismatch, save the printed reproducer as a fixed
  regression `TEST` when possible, and retain the file reproducer (printed
  values can be truncated). Fix the Rust, then re-run BOTH that regression
  test and the campaign.

Coverage guidance comes from the C reference (it is the instrumented side); the
Rust translation is executed as a black box on every input, so any normalized
mismatch is still caught and reported through the usual GoogleTest assertion.
Because the campaign steers by the C reference and treats it as ground truth, a
misbuilt C reference is especially damaging here: it will happily "find" a flood
of failing inputs that are really the oracle's fault, not the translation's.
Before starting a campaign, make sure the C side is compiled correctly (see the
compile-definitions check in Step 3) — the instrumented, statically linked C in
`c_under_test` must match how `c_src` is actually built.

Crash handling: fuzzing runs in-process, so a segfault or abort on either side
ends the run. Treat the cases distinctly — if the **C reference** itself crashes
or trips a sanitizer on an input, that is a reference-side issue (record it, do
not conclude the Rust is wrong); if C completes (returning success or a normal
error) and Rust diverges or crashes, that is a translation bug.

Bound input sizes, output expansion, and work per input. Free both sides' contexts
on every iteration, including error paths. An OOM can reflect cumulative growth,
so check retained state as well as the last input.
