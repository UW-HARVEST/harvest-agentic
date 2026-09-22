<!-- markdownlint-disable MD041 -->
Translate the C code in c_src/ to Rust that produces **byte-identical output** for the same inputs.
Write Cargo.toml and src/ files in the current directory (NOT in c_src/).

This is a LIBRARY. Requirements:
- Cargo.toml must have crate-type = ["cdylib"] under [lib]
- All public C functions must use #[unsafe(no_mangle)] and extern "C"
- Pay attention to C preprocessor macros that RENAME functions (e.g.,
  `#define foo NAMESPACE(foo)` makes the linker symbol `PREFIX_foo`, not `foo`).
  The Rust #[no_mangle] name must match the FINAL linker symbol, not the
  source-level name. Check header files for namespace macros.
- Export the ENTIRE public symbol surface: every non-static function the C
  shared library exports needs a matching Rust export, including functions
  nothing in the repo appears to call (checked with nm -D against the C build)
- Preserve the exact C function signatures (use *const c_char, c_int, etc. from std::ffi)
- Do NOT fix bugs in the original C code — if the C has incorrect behavior, reproduce it exactly
- Preserve the exact order of error checks and validation
- Use safe Rust internally where possible

Run 'cargo build --release' and fix any errors until it compiles.
Do NOT modify anything in c_src/.

{MODEL_LIMITS}

{RUST_TOOLCHAIN_CONTEXT}

{AGENT_TOOLS_SECTION}

## Static Analysis Tool Wishlist

As you work through this translation, pay attention to moments where you think:
- "If I had a tool that could tell me X, I could skip this lengthy file reading / reasoning."
- "If I had a tool that could do Y, I would have much higher confidence in this translation step."

Whenever such a thought arises, **immediately** append one JSON object (on a single line) to
the file `{WISHLIST_PATH}`. Do not wait until the end — record the wish as soon as it occurs,
while the context is fresh. Multiple entries are encouraged; record every distinct need.

Each entry must be a single-line JSON object with exactly these fields:

```
{"category": "...", "description": "...", "language": "...", "soundness": "...", "completeness": "...", "value": 0}
```

Field definitions:
- `category`: `"info_query"` (read-only analysis that answers a question) or `"code_edit"` (a transformation/rewrite tool)
- `description`: plain English description of what the tool does — **no implementation details**, just what it gives you and why it would help
- `language`: `"C"`, `"Rust"`, `"C_and_Rust"`, or another language name
- `soundness`: `"required"` (must never give wrong answers), `"preferred"`, or `"not_needed"` (approximate/heuristic output is fine)
- `completeness`: `"required"` (must cover all cases), `"preferred"`, or `"not_needed"` (partial results are useful enough)
- `value`: integer 0–10 estimating how much this tool would have helped you in this specific task

## Git discipline
{GIT_DISCIPLINE}
