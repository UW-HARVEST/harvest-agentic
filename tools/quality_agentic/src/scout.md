# Global scout task

You investigate why a group of unsafe operations in a Rust crate (translated from C) cannot be
removed one function at a time, you propose a radical change of the data model that removes them,
and you lay down the proposal's contract. The main agent chooses among the proposals of all
scouts; implementers then carry out the chosen ones, in parallel, on branches made from yours, and
every result must pass the regression gate. So be bold about the design and exact about the
contract, the plan and the risks.

Your task message gives: your cluster (the root causes to investigate), your checkout (the
worktree `wt/p<n>` on branch `p<n>`; run every command from its root), the scan
(`shared/quality/<name>`), the roots report, and your proposal number. `S` below means
`inputs/agent_skills/rust_quality/scripts`.

{RULES}

Also, for this task:

- Read `inputs/agent_skills/rust_quality/GLOBAL.md` first, and `CURSORS.md` when your cluster
  has buffers walked with pointers (cursors).
- Change the crate only by the additive contract (the "Contract" step below). Commit only on your
  branch.
- Use the measurements; do not guess counts. `python3 S/qscan.py fn <name> --scan <scan>` lists the
  unsafe operations of a function; the scan's `ops.tsv` has one line per operation with its
  variable and type; `functions.tsv` has one line per function. To check an expected gain, you may
  convert 2 or 3 representative functions in a copy of the crate:
  `rsync -a --exclude target --exclude wt --exclude scratch --exclude .git --exclude .opencode ./ scratch/scout/crate/`,
  then `cargo check` there. Always exclude `scratch`: the copy goes into `scratch/`, and without
  that exclude rsync copies the copy into itself again and again and fills the disk. Do not
  implement the whole proposal: that is the implementers' work.

## Investigate

1. For your cluster: which types, fields, buffers and functions cause the operations; how they
   flow through the call graph (`grep -n` for callers); which part is forced by the C ABI.
2. What C can see: read the public headers (`inputs/c_src/**/include/*.h` and the headers that the
   C library installs). A struct that C callers allocate or see is pinned.
3. What the tests cover: `grep -n` the exported functions of your cluster in `verify_env/` (and
   `tests/`). Untested code is risky to change.
4. Hazards: pointers into the same object, pointer identity checks, writes past the logical end,
   overlapping copies (in-place use that the C API allows), custom allocators and callbacks,
   allocation failure paths.

## Propose

Write `quality/proposals/P<n>.md` (use the heredoc form if the Write tool refuses:
`cat > FILE <<'EOF'`). Start the file with this header table, filled with numbers:

```markdown
# P<n>: <one-line title>

| Field | Value |
|---|---|
| Cluster | <root causes it removes> |
| Techniques | <G1..G8 from GLOBAL.md, the cursor model, and anything new> |
| Expected unsafe operations removed | <number> of <cluster total> (method: <how you counted>) |
| Expected lints / idiom effect | <e.g. -N as casts, Result instead of int codes in M functions> |
| Files and functions touched | <count> files, <count> functions: <list or ranges> |
| Types C can see that change | none (required) |
| Test coverage of the touched code | <tests and fuzz properties that call it; gaps> |
| Risk | low / medium / high: <the main way it could change behavior> |
| Sub-tasks | <count>, parallel: <which can run at the same time> |
```

Then these sections:

1. **Root cause**: with evidence (file:line, operation counts per function).
2. **Design**: the new data model, in prose and in Rust. For buffers and cursors, the cursor table
   and the checklist answers of `CURSORS.md`.
3. **Contract**: the exact Rust code that you add in the next step: new types and modules, trusted
   primitives with their `// SAFETY:` invariants, changed internal signatures, adapter functions
   at the C boundary.
4. **Sub-tasks**: `S1`, `S2`, ...: for each, the functions or files it owns, what it changes,
   which sub-tasks it waits for, and how to check it (which tests and fuzz properties exercise
   it). Each sub-task must leave the crate building and passing the gate on its own branch (keep
   old entry points as shims until the last sub-task, if needed).
5. **Hazards and how the plan handles each one.**
6. **What stays unsafe and why.**
7. **Alternatives you rejected**: one line each.

## Contract

1. Add the contract to the crate: the new modules, types, trusted primitives and adapter
   functions of the "Contract" section. It is additive: existing code does not use it yet, unless
   the proposal says so. Behavior does not change.
2. `cargo build --release`, then `python3 S/regress.py check --quick` (it must end with
   `REGRESS: OK`; see `SKILL.md`, "Long commands").
3. Commit the proposal file and the contract: `git add -A && git commit -m "P<n>: proposal and
   contract"`. If the contract does not build, correct it and the "Contract" section together.

## Reply

At most 25 lines: the header table values, the one sentence that says why the proposal is worth
the risk, and the last line of the gate check.
