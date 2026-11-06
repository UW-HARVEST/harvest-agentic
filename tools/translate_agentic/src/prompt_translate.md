# C → Rust Capsule Planner

**This is a planning-only task. Do not translate any code, write any Rust,
add any build guards, or modify the C source. Your only deliverable is a
written plan.** Analyze the C library, work out how it should be broken into
capsules for a later C → Rust migration, and print that plan to the
terminal.

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
functions that will
later be translated to Rust together, as one unit, and flipped from C to
Rust all at once — never half-in-Rust. Inside a capsule, Rust functions can
call each other however is natural because nothing outside the capsule
calls them directly. Only two things cross the C boundary:

1. A fixed set of `extern "C"` **trampoline** functions — one per name that
   outside code already calls by that exact symbol.
2. One internal entry surface that every trampoline funnels through.

The list of required trampoline names per capsule isn't a design choice —
it's whatever the real external call graph already requires. Your plan's
job is to work out those capsule boundaries and boundaries' trampoline
names from the actual code, so the later migration agent doesn't have to.

## Step A: Generate the call graph

Start by generating the project's call graph with the `callgraph` tool. It
compiles the project to LLVM IR and reports, for every function, which
functions it calls. This is the backbone of the whole plan.

```
python3 {AGENT_TOOLS_DIR}/callgraph.py list <compile_commands.json>
python3 {AGENT_TOOLS_DIR}/callgraph.py from <compile_commands.json> <function> [--depth N]
python3 {AGENT_TOOLS_DIR}/callgraph.py dot  <compile_commands.json>
```

- **Requires `compile_commands.json`** for the project (generate it with
  CMake `-DCMAKE_EXPORT_COMPILE_COMMANDS=ON`, or `bear -- make`). All
  source files must compile; files that fail are skipped with a warning,
  so a sparse graph usually means files silently dropped out — check the
  warnings and note any skipped files in the plan.
- **`list`** gives every function and its direct callees — the fastest way
  to read off who calls what across the whole project.
- **`from <function>`** gives the call tree rooted at one function — use it
  to trace a public entry point down through its dependencies.

Two things about this tool the plan depends on:

- **Function pointers are NOT resolved.** A call made through a function
  pointer produces no edge. Instead, any function that contains such a call
  is flagged `[indirect call site — callees unresolved]`. Resolving what
  the pointer points to is undecidable in general, so the tool leaves it to
  you. **A flagged function is exactly where the static graph is
  incomplete, and it may hide a real cross-capsule call edge.** When you
  hit a flag, read the surrounding code to work out the possible targets
  (where the function pointer is assigned — a struct field, a dispatch
  table, an argument), and record any target in another file as a real
  external boundary. Do not treat a flagged function as a leaf. To get the
  full list of flagged functions quickly, pipe the `dot` output through
  grep: `python3 {AGENT_TOOLS_DIR}/callgraph.py dot <compile_commands.json>
  | grep "indirect-call-site"`.
- **`static` functions with the same name in different files** appear with
  a numeric suffix (e.g. `LZ4_read32.2`). Treat `foo` and `foo.N` as the
  same source function; the suffix just means the function is `static` and
  duplicated across translation units.

The call graph is a strong heuristic, not ground truth — it won't show
indirect calls, and it stops at any prebuilt library you only link against
(libc, etc.). Use it to do most of the boundary-finding fast, then confirm
the load-bearing edges against the source.

## Step B: Find the real capsule boundaries

Don't chunk by source order or file size — find the boundaries the C code
already has. Working from the call graph:

- **Classify every function as `static` or externally linkable.** `grep`
  for `static` on each function definition. `static` functions have no
  linker symbol — they can only be reached from within their own
  translation unit, the strongest signal of "never meant to be
  independently callable." (The call graph's suffixed names, e.g. `foo.2`,
  flag which functions are `static` and duplicated.)
- **Watch for source-level `#include` of `.c` files** (a unity build /
  amalgamation — one file does `#include "other.c"` instead of linking
  against it). This makes a file boundary compile-time-only: the includer
  gets its own private copy, with no real linker dependency. Check whether
  the includer pulls in only a *subset* behind a guard macro (as `lz4hc.c`
  does with `LZ4_COMMONDEFS_ONLY`) — that guard tells you which functions
  are "shared primitives" vs "this file's own logic."
- **For every externally-linkable (non-`static`) function, find its real
  callers** — read them off the call graph first, then grep to confirm,
  and check whether it's in a public header vs. gated behind an
  "advanced/internal use" macro (LZ4 uses `LZ4_STATIC_LINKING_ONLY`). A
  function only ever called from inside its own file, despite external
  linkage, still belongs in that file's capsule — the actual call site is
  the proof, not the header declaration.
- **Recover the edges the call graph flagged.** For every function marked
  `[indirect call site — callees unresolved]`, resolve its function-pointer
  target(s) from the source and record those call edges before drawing
  boundaries. A dispatch through a pointer to a function in another file is
  a real cross-capsule dependency the static graph could not see — miss it
  and the later migration draws a boundary that breaks at link time.
- **Group by "who legitimately calls this from outside its own file."**
  Everything reachable only from within one file (or one guarded region) is
  one capsule's internals. A function called from a different file is part
  of that capsule's external boundary, and its exact name + signature
  becomes a required trampoline.
- **Draw the capsule dependency graph** from those real call edges (call
  graph edges plus recovered indirect ones, not file names) and
  topologically sort it. Capsules with nothing to wait on go first.

If a set of `static` helpers is reused by more than one capsule (via a
unity-build `#include`, or duplicated/copy-pasted — the suffixed `foo.N`
names are a signal of this), plan for it as its own small shared-primitives
capsule with no external trampoline names (nothing links to it today) and
a short-lived internal-only gateway, to be deleted once every consumer
capsule is migrated. Note this in the plan.

## Step C: What the plan must contain

Print the plan as Markdown to standard output. It must include:

1. **Overview** — the library, its source files, and the total function
   count. Note any files the call graph skipped (failed to compile) and any
   prebuilt libraries the graph stops at.
2. **Capsule list, in migration (topological) order.** For each capsule:
   - **Name** and a one-line description of what it does.
   - **Member functions** — the full list from Step B's classification.
   - **Dependencies** — which other capsules it calls into.
   - **External boundary** — the exact symbol names + signatures other
     capsules/files/tests call it by today. These become the required
     `extern "C"` trampolines. Read off the real call graph, not designed.
   - **ABI-fixed data** — any struct a caller allocates directly (stack,
     heap, or embeds by value) rather than receiving as an opaque pointer.
     Its `#[repr(C)]` layout is part of the contract; flag it so the later
     migration pins it before writing logic.
   - **Suggested chunking** — order the capsule's own functions by internal
     dependency (leaf helpers first, public entry points last) and slice
     into chunks of ~10-15 functions for later per-chunk translation. This
     is context-sizing only, not an FFI boundary.
3. **Indirect-call findings** — a dedicated list of every function the call
   graph flagged, the target(s) you resolved for each, and how you resolved
   them (which struct field / dispatch table / assignment). This is the
   part of the plan most likely to be wrong, so make it explicit and
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