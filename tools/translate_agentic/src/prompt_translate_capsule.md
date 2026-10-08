# Generic C → Rust Translation Plan v2

Goal: turn any C library into Rust, testing as you go, complete runnable
library at every step. Unit of translation: **capsules** instead of
individual functions.

## The big idea

A capsule is a cluster of C functions translated to Rust together, as one
unit, that flips from C to Rust all at once — never half-in-Rust. Inside a
capsule, Rust functions can call each other however is natural (borrows,
`Result`, owned types, whatever) because nothing outside the capsule calls
them directly. Only two things cross the C boundary:

1. A fixed set of `extern "C"` **trampoline** functions — one per name that
   outside code already calls by that exact symbol. Each is a mechanical
   one-line forward, nothing to design.
2. One internal entry surface that every trampoline funnels through. This
   is the only place in the capsule where the C boundary (ownership,
   lifetimes, panics) needs real thought.

The list of required trampoline names per capsule isn't a design choice —
it's whatever the real external call graph already requires (see below).

## Tool: the call graph generator

Before finding capsule boundaries by hand, generate the project's call
graph with the `callgraph` tool. It compiles the project to LLVM IR and
reports, for every function, which functions it calls. This is the "real
call graph" that Steps 1, 2, and 6 depend on — use it as the primary
source of call edges, and use `grep` only to confirm or fill gaps.

```
python3 {AGENT_TOOLS_DIR}/callgraph.py list <compile_commands.json>
python3 {AGENT_TOOLS_DIR}/callgraph.py from <compile_commands.json> <function> [--depth N]
python3 {AGENT_TOOLS_DIR}/callgraph.py dot  <compile_commands.json>
```

- **Requires `compile_commands.json`** for the project (generate it with
  CMake `-DCMAKE_EXPORT_COMPILE_COMMANDS=ON`, or `bear -- make`). All
  source files must compile; files that fail are skipped with a warning,
  so a sparse graph usually means files silently dropped out — check the
  warnings.
- **`list`** gives every function and its direct callees — the fastest way
  to read off who calls what across the whole project.
- **`from <function>`** gives the call tree rooted at one function — use it
  to trace a public entry point down through its dependencies.

Two things about this tool the plan depends on, so read them carefully:

- **Function pointers are NOT resolved.** A call made through a function
  pointer produces no edge in the graph. Instead, any function that
  contains such a call is flagged
  `[indirect call site — callees unresolved]`. Resolving what the pointer
  actually points to is undecidable in general, so the tool deliberately
  leaves it to you. **A flagged function is exactly where the static graph
  is incomplete, and it may hide a real cross-capsule call edge.** When you
  hit a flag, read the surrounding code to work out the possible targets
  (look at where the function pointer is assigned — a struct field, a
  dispatch table, an argument), and treat any target that lives in another
  file as a real external boundary that needs a trampoline. Do not treat a
  flagged function as a leaf. In DOT output these are also emitted as
  colored nodes with a `// indirect-call-site:` comment, so
  `grep "indirect-call-site"` on a `.dot` file lists them all.
- **`static` functions with the same name in different files** appear with
  a numeric suffix (e.g. `LZ4_read32.2`, `LZ4_read32.6`). Treat `foo` and
  `foo.N` as the same source function — the suffix is just LLVM keeping
  duplicate `static` definitions distinct. (This overlaps with the `static`
  classification in Step 1: a name appearing with suffixes is `static` and
  duplicated across translation units.)

The call graph is a strong heuristic, not ground truth — it will not show
indirect calls, and it stops at the boundary of any prebuilt library you
only link against (libc, etc.). Use it to do most of the boundary-finding
fast, then confirm the load-bearing edges (especially around flagged
functions) against the source.

## Step 1: Find the real capsule boundaries

Don't chunk by source order or file size — find the boundaries the C code
already has. Start from the call graph (above), then for the specific
project:

- **Classify every function as `static` or externally linkable.** `grep`
  for `static` on each function definition. `static` functions have no
  linker symbol at all — they can only be reached from within their own
  translation unit. That's a hard boundary already enforced by the
  compiler, and it's the strongest signal of "this was never meant to be
  independently callable." (The call graph's suffixed names, e.g. `foo.2`,
  flag which functions are `static` and duplicated across files.)
- **Watch for source-level `#include` of `.c` files** (a "unity build" /
  amalgamation pattern — one file does `#include "other.c"` instead of
  linking against it). This turns what looks like a file boundary into a
  compile-time-only one: the includer gets its own private copy of
  whatever it pulls in, and there's no real linker dependency there at
  all. Check both directions — does the includer only pull in a *subset*
  behind a guard macro (as `lz4hc.c` does with `LZ4_COMMONDEFS_ONLY`)? If
  so, that guard is telling you exactly which functions are "shared
  primitives" vs "this file's own logic."
- **For every externally-linkable (non-`static`) function, find its real
  callers** — read them off the call graph first (the `from` subcommand
  traces callers of an entry point; `list` shows every edge), then grep for
  its name across every other `.c` file to confirm, and check whether it's
  declared in a public header vs. gated behind a project-specific
  "advanced/internal use" macro (LZ4 does this with
  `LZ4_STATIC_LINKING_ONLY`). A function only ever called from inside its
  own file, despite technically having external linkage, still belongs
  inside that file's capsule — the header declaration alone isn't proof of
  a real cross-capsule dependency, the actual call site is.
- **Recover the edges the call graph flagged.** For every function marked
  `[indirect call site — callees unresolved]`, resolve its function-pointer
  target(s) from the source and add those call edges by hand before
  drawing capsule boundaries. A dispatch made through a pointer to a
  function in another file is a real cross-capsule dependency that the
  static graph could not see — miss it and you will draw a boundary that
  breaks at link time.
- **Group by "who legitimately calls this from outside its own file."**
  Everything reachable only from within one file (or one guarded region of
  one file) is one capsule's internals. A function called from a different
  file is part of that capsule's real external boundary, and its exact
  name + signature becomes a required trampoline.
- **Draw the dependency graph between capsules** from those real call
  edges (call-graph edges plus the recovered indirect ones, not file
  names) and topologically sort it. Capsules with no incoming requirement
  to wait on anything go first.

If a set of `static` helpers is reused by more than one capsule (via a
unity-build `#include`, or by being duplicated/copy-pasted — the call
graph's suffixed `foo.N` names are a signal of this too), pull it out into
its own small capsule with no fixed external trampoline names (nothing
links to it today) and give it a short-lived internal-only gateway so
already-migrated consumer capsules can reach it in Rust while other
consumers are still C. Delete that gateway once every consumer capsule is
migrated — see the PRIM capsule in the LZ4 worked example for exactly this
situation.

## Step 2: Define each capsule

For every capsule, write down:
- **Member functions** — the full list, taken from Step 1's classification.
- **Dependencies** — which other capsules' gateways it calls into, read off
  the call graph (including any indirect edges recovered in Step 1).
- **External boundary** — the exact symbol names + signatures other
  capsules/files/tests call it by today. This is what the trampolines must
  cover; it's read off the real call graph, not designed.
- **ABI-fixed data**: any struct a caller allocates directly (stack, heap,
  or embeds by value in its own struct) rather than getting back as an
  opaque pointer. Its layout is part of the contract — pin the `#[repr(C)]`
  shape once, before writing any of that capsule's logic, and don't touch
  it again.

## Step 3: Break each capsule into chunks

Order the capsule's own functions by internal dependency — leaf helpers
first, functions that build on those next, the big public entry points
last — then slice into chunks of roughly 10-15 functions. The call graph's
`from` subcommand, run on a capsule's entry points, gives this internal
ordering directly. This is purely for agent-context sizing; a chunk
boundary is not an FFI boundary and never gets its own C-callable symbol,
since nothing outside the capsule can see or call a half-finished
capsule's internals.

## Step 4: How a capsule flips

A shared library (or static library / single binary — same idea, just
picking which object file's definition wins at link time) can't hold two
definitions of the same symbol, so the switch is all-or-nothing per
capsule, not per function:

1. Wrap each capsule's C bodies behind one build guard for that capsule
   (e.g. `#if !RUST_<CAPSULE_NAME>` around its whole region) — one guard
   per capsule, not one per function.
2. When a capsule's guard is on, its C bodies drop out of the build and the
   Rust side's `extern "C"` trampolines (same names, same signatures)
   satisfy the linker instead. Nothing outside the capsule needs to change
   — it was already calling these functions by name through the header.
3. Only flip a capsule once everything it depends on is already-flipped
   Rust or still fully-working C — follow the dependency order from Step 1.

## Step 5: Test as you go

If the project already has a test suite / fuzzer harness that only
exercises the public API (true for most C libraries), it needs **no
adaptation** to validate a partially-migrated build — a build with only one
capsule's guard on is indistinguishable from full-C to anything calling
through the public headers. If no such suite exists, build one incrementally
alongside the translation rather than skipping this step — it's what makes
every later capsule's flip checkable.

For every chunk inside a capsule:
1. Translate into Rust behind the capsule's still-private internal API.
2. Generate reference input/output pairs by running the *original* C
   function(s) being replaced — a small throwaway C harness (using the
   same textual-`#include` trick as Step 1 if the functions are `static`)
   — and assert the new Rust code reproduces them exactly. Pure Rust-side
   testing: no FFI, no library rebuild, since nothing outside the capsule
   can see this yet.
3. Only move to the next chunk once it passes.

Once every chunk in a capsule is done:
1. Build the library with the capsule's guard on and off.
2. Run the full test suite against both and diff.
3. Run any available fuzzers against the guard-on build, targeting this
   capsule's boundary inputs specifically. Pay special attention to any
   boundary the call graph flagged as an indirect call site — those edges
   were recovered by hand and are the likeliest place a real callee was
   missed.
4. Keep the guard-off path buildable until this passes — it's the rollback.

## Step 6: Use sub-agents to manage limited context

Four roles per capsule:
- **Contract agent** (first, once per capsule): reads the capsule's
  functions and every real external call site (from the call graph, plus
  the indirect edges recovered in Step 1), writes down the gateway
  contract — internal entry surface, fixed trampoline names/signatures,
  any ABI-fixed struct layouts. Reviewed before any Rust is written, since
  a mistake here is expensive to unwind later. Where the call graph flagged
  an indirect call site inside this capsule, the contract must state which
  targets were resolved and how, so the reviewer can check them.
- **Implementation agents** (one per chunk, can run in parallel once the
  contract is fixed): translate against the fixed contract, produce the
  reference-vector tests from Step 5.
- **Integration agent** (once per capsule): writes the trampolines, flips
  the build guard, runs the capsule-level testing from Step 5.
- **Review agent** (independent of the implementer, once per capsule):
  checks translation fidelity per chunk, and that the gateway was actually
  honored — no stray `pub extern "C"` leaked beyond the agreed trampoline
  list, no ownership/lifetime crossing the boundary the contract didn't
  account for, no panic path that can unwind across `extern "C"`, and that
  every function-pointer target the contract claimed to resolve is real.

Keep a running checklist of capsule status (not started / contract written
/ chunks in progress / done / flipped+tested) — the only thing that needs
to carry over between agent handoffs.

## Step 7: Finishing up

Once every capsule is flipped and tested, delete every build-guard option
and any temporary shared-primitives gateway from Step 1, then run the full
test suite one more time against the all-Rust build to confirm the
capsules work together, not just each against its own boundary tests.