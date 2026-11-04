# Quality worker task

You make the functions of one unit of a Rust crate safer and more idiomatic, without changing
their behavior. The crate was translated from C; it passes its tests. A script merges your branch
only when the regression gate still passes, so a change that breaks a test is wasted, and an
attempt that fails and is undone costs little.

Your task message gives:

- your checkout (a git worktree; run every command from its root, never `cd` elsewhere),
- your unit: files, line ranges of large files, and its function names,
- the scan to start from (`shared/quality/<name>`).

`S` below means `inputs/agent_skills/rust_quality/scripts`. `SKILL.md` describes the tools; read
its sections "The tools" and "Long commands" first.

{RULES}

Also, for this task:

- Change only the functions of your unit. Do not change `use` lines, other functions, other
  files or `Cargo.toml`. A recipe that needs a change outside your unit: use its in-body or shim
  form, or skip it and name the change in your report.
- You run `regress.py check --quick` and `qscan.py scan/file/fn/compare`. You do not run
  `regress.py setup` or `qscan.py autofix`.
- Commit only on your branch, in your worktree. Do not merge. Do not remove the worktree.

## Steps

1. **Worklist.** For each file of your unit:

   ```
   python3 S/qscan.py file <file> --scan <the scan from your task> [--lines <your range>]
   ```

   It lists the functions with work, highest score first, with their facts, the lines of their
   unsafe operations, the pointer parameters that are R3 candidates, their lints and the suggested
   recipes. If lines moved since that scan, run `python3 S/qscan.py scan --out scratch/qscan_start`
   first and use that scan.

2. **Read the recipes** that the worklist suggests in `inputs/agent_skills/rust_quality/RECIPES.md`.
   You must read R3 and R4 in full before you decide about any function that the worklist suggests
   them for.

3. **Must-try list.** The 8 functions with the most unsafe operations in your unit (or all, if
   fewer) must each get an attempt at every suggested R3/R4/R12 recipe. For a recipe that you
   skip, write the failing condition number and the line that shows it. A skip without a named
   condition is not allowed.

4. **Work in batches of 2 or 3 functions**, highest score first:
   1. Read each whole function: `sed -n '<first>,<last>p' <file>`.
   2. Apply its suggested recipes in this order: R12, R1, R2, R3/R4, R6, R5, R7, R8, R11, R9, and
      R10 last (document only the unsafe that is left).
   3. `cargo build --release 2>&1 | grep -E '^(error|warning: unused)' -A5 | head -40`.
      An error in a file outside your unit: undo the change that caused it, or use the in-body form.
   4. `python3 S/regress.py check --quick`. The last line must be `REGRESS: OK`.
      - `REGRESS: FAIL`: read `scratch/regress/last.md`. Undo the batch (`git checkout -- <files>`),
        then redo its functions one at a time with a check after each, to find the one that fails.
        Fix that one in at most 2 attempts, else undo it and record it as "undone".
   5. Commit the batch: `git commit -qam "quality: <functions>: <recipes>"`.

5. **Finish.**
   1. `python3 S/qscan.py scan --out scratch/qscan_end`
   2. `python3 S/qscan.py compare <the scan from your task> scratch/qscan_end --files <your files>`.
      Fix every line under PROBLEMS, or undo the function that caused it.
   3. `python3 S/regress.py check --quick`. It must end with `REGRESS: OK`.
   4. Commit. `git status` must be clean.

## Report

Reply with only this, at most 45 lines:

```
Unit <id>: <files>
Before -> after (your files, from qscan compare): unsafe ops A -> B, lints C -> D, extra lints E -> F, pseudo-safe G -> H
Recipes applied: R3 x<n> (<function names>), R4 x<n> (...), ...
Attempted and undone: <function>: <recipe> — <what failed> (one line each)
Skipped (must-try list only): <function>: <recipe> — condition <n>, <file:line> (one line each)
Changes needed outside this unit (not done): <function and what>, or none
Last regress.py check: <its last line>
Commits: <number>; git status clean: yes/no
```
