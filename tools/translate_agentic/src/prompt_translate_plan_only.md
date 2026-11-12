# C → Rust Capsule Planner

**This is a planning-only task. Do not translate any code, write any Rust,
add any build guards, or modify the C source. Your only deliverable is a
written plan.** Analyze the C library, use the `capsules` tool to work out
how it should be broken into capsules for a later C → Rust migration, and
print that plan to the terminal.

The migration itself (translating capsules, flipping build guards, testing)
is a separate task that a different agent will carry out later, using the
plan you produce. Your job stops at the plan.

## Where to write the plan

**Print the finished plan to the terminal (standard output) as a single
Markdown document.** Do not write it to a file, and do not modify anything
in the project source tree — this task only reads the C code and emits a
plan. The plan is the entire terminal output; anything you print before it
(tool progress, notes) should go to standard error or be clearly separated,
so the plan itself can be captured cleanly from stdout.

## What a capsule is (context for the plan)

The migration unit is a **capsule** (this is background so your plan makes
sense to whoever reads it — you are not building any of it): a cluster of C
functions that will later be translated to Rust together, as one unit, and
flipped from C to Rust all at once — never half-in-Rust. Inside a capsule,
Rust functions can call each other however is natural because nothing
outside the capsule calls them directly. Only two things cross the C
boundary:

1. A fixed set of `extern "C"` **trampoline** functions — one per name that
   outside code already calls by that exact symbol.
2. One internal entry surface that every trampoline funnels through.

The list of required trampoline names per capsule isn't a design choice —
it's whatever the real external call graph already requires. The `capsules`
tool works the capsule boundaries and their trampoline names out from the
actual code, so you don't have to do that grouping by hand — your job is to
run it, then fill in and verify the parts it cannot know (see Step C).

## Step A: Run the capsules tool

The `capsules` tool does the mechanical work: it runs the call graph
internally, groups the functions into capsules (clusters that mostly call
among themselves), orders the capsules so dependencies come first, computes
each capsule's external boundary (the trampoline names), and prints a
Markdown plan — including a quality scorecard and the unresolved
function-pointer flags carried straight through from the call graph.

```
python3 {AGENT_TOOLS_DIR}/capsules.py <compile_commands.json> \
    --callgraph {AGENT_TOOLS_DIR}/callgraph.py --max-cross 0.5
```

- **Requires `compile_commands.json`** for the project (generate it with
  CMake `-DCMAKE_EXPORT_COMPILE_COMMANDS=ON`, or `bear -- make`). It also
  needs `callgraph.py` (pass its path with `--callgraph`) and that tool's
  own dependencies (`clang`, `llvm-link`, `opt`, `llvm-dis`).
- **The plan prints to standard output.** The tool's progress and any
  warnings go to standard error, so `> plan.md` captures just the plan.
- **`--max-cross F`** (0.0–1.0) is the one grouping knob: the target
  ceiling on the fraction of all calls allowed to cross a capsule boundary.
  Lower → fewer, larger, more self-contained capsules; higher → more,
  smaller ones. Start at the default (0.5), read the scorecard, and retune
  only if the capsules come out too large or too fragmented to hand off.

**Read the tool's stderr before trusting the plan.** It surfaces
`callgraph.py`'s own warnings:

- A `SKIP` line means a source file failed to compile and was left out, so
  the graph (and the plan) is missing those functions. Note any skipped
  files in your Overview.
- A `WARN: llvm-dis` line means the function-pointer flags were NOT produced
  — so an empty "unresolved function pointers" list would be meaningless,
  not reassuring. Fix the toolchain (ensure `llvm-dis` is on PATH and the
  same LLVM version as `clang`/`opt`) and rerun before writing the plan.

## Step B: What the tool does NOT do — and you must

The `capsules` tool groups by the *static* call graph. That graph has two
blind spots, and the plan is only trustworthy once you have closed them by
reading the source. Do this before finalizing anything.

- **Resolve the flagged function pointers.** Any function the plan marks
  **[indirect]** contains a call through a function pointer whose target the
  static graph could not name. These are exactly where the grouping may be
  wrong: a dispatch through a pointer to a function in another capsule is a
  real cross-capsule edge the tool could not see, and missing it means a
  boundary that breaks at link time. For each flagged function, read the
  surrounding code, work out the possible targets (where the pointer is
  assigned — a struct field, a dispatch table, a function argument), and if
  any target lives in another capsule, record it as a real boundary. To list
  every flagged function directly from the call graph:
  `python3 {AGENT_TOOLS_DIR}/callgraph.py dot <compile_commands.json> | grep "indirect-call-site"`.
- **Check the `static`-duplicate names.** A function that appears with a
  numeric suffix (e.g. `LZ4_read32.2`) is a `static` function duplicated
  across translation units — `foo` and `foo.N` are the same source
  function. If such a helper is shared by more than one capsule (via a
  unity-build `#include` of a `.c` file, or copy-paste), plan for it as its
  own small shared-primitives capsule with no external trampoline names
  (nothing links to it today) and a short-lived internal-only gateway, to be
  deleted once every consumer capsule is migrated. Note this in the plan.
- **Spot-check the boundary against the source.** The tool lists each
  capsule's external boundary from call edges. Confirm a couple of them: a
  boundary function is one actually called from another capsule's code. A
  function that merely appears in a public header but is only ever called
  from inside its own capsule is not a real boundary — the call site is the
  proof, not the declaration.

Treat the tool's output as a strong first draft of the boundaries, not
ground truth. Most of it will be right; the indirect-call edges are the part
most likely to need correction.

## Step C: What the plan must contain

Print the plan as Markdown to standard output. Build it from the tool's
output plus your Step B findings. It must include:

1. **Overview** — the library, its source files, and the total function
   count. Note any files the call graph skipped (failed to compile, from the
   tool's stderr) and any prebuilt libraries the graph stops at (libc, etc.).
   Include the tool's quality scorecard (cohesion / cross-ratio / modularity)
   and the `--max-cross` value you used, so a reader can judge the grouping.
2. **Capsule list, in migration (topological) order** — as the tool emits
   it, with these additions per capsule that the tool cannot produce:
   - **Name / description** — the tool labels capsules `C1`, `C2`, … Give
     each a meaningful name and a one-line description of what it does, read
     off its member functions.
   - **Member functions** — from the tool.
   - **Dependencies** — from the tool (which other capsules it calls into),
     corrected for any indirect edges you recovered in Step B.
   - **External boundary** — from the tool (the trampoline names), plus the
     exact signatures, which you read from the source.
   - **ABI-fixed data** — any struct a caller allocates directly (stack,
     heap, or embeds by value) rather than receiving as an opaque pointer.
     Its `#[repr(C)]` layout is part of the contract; flag it so the later
     migration pins it before writing logic. The tool does not detect this —
     you find it by reading the code.
   - **Suggested chunking** — order the capsule's own functions by internal
     dependency (leaf helpers first, public entry points last) and slice
     into chunks of ~10–15 functions for later per-chunk translation. This
     is context-sizing only, not an FFI boundary.
3. **Indirect-call findings** — a dedicated list of every function the plan
   flagged **[indirect]**, the target(s) you resolved for each, and how you
   resolved them (which struct field / dispatch table / assignment). This is
   the part of the plan most likely to be wrong, so make it explicit and
   reviewable rather than folding it silently into the capsule edges.
4. **Shared-primitives capsule(s)**, if any, with the gateway-and-delete
   note from Step B.
5. **Open questions / risks** — anything you could not resolve from static
   analysis alone (unresolved function pointers, ambiguous ownership at a
   boundary, structs whose layout you're unsure is caller-visible). Flag
   these for a human or the migration agent rather than guessing.

## Step D: Finish

The Markdown plan printed to standard output is the entire deliverable. Do
not begin translating and do not write the plan (or anything else) to disk
— the migration is a separate task that a later agent will run from this
plan.