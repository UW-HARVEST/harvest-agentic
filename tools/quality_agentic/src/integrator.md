# Quality integrator task

You merge the branches of other sub-agents into one integration branch and keep only what passes
the regression gate. The main agent merges or accepts your branch only after it checked your
report and your last gate result, so the report must be exact.

Your task message gives:

- your checkout: the worktree of the integration branch (run every command from its root),
- the mode: **proposal** (the sub-task branches of one proposal, merged into the proposal branch
  `p<n>`) or **final** (the accepted proposal branches, merged into `final<r>`),
- the branches to merge, in order (in final mode there can be none),
- the "before" scan (`shared/quality/<name>`) and the name of the "after" scan to write,
- final mode only: `finish: yes` or `finish: no`.

`S` below means `inputs/agent_skills/rust_quality/scripts`.

{RULES}

Also, for this task:

- Read `inputs/agent_skills/rust_quality/GLOBAL.md` (and the proposal files of the branches)
  before you fix anything. For a proposal with a cursor table, check the merged code against the
  table as the last section of `CURSORS.md` says, and fix or reject what does not match.
- You may fix the joints between branches (a conflict, a build error, a regression that two
  branches cause together), with at most 2 fix attempts per branch. Commit each fix as
  `integrate: <what>`. If the fix does not work, reject the branch.
- Commit only on your integration branch. Do not touch `main`. Do not remove worktrees.

## Steps

1. For each branch, in the given order:
   1. Note the current commit: `git rev-parse HEAD` (call it R).
   2. `git merge --no-ff -m "Merge <branch>" <branch>`. A conflict: resolve it when it is small
      and you understand both sides, then `git commit --no-edit`; else `git merge --abort` and
      reject the branch.
   3. `cargo build --release 2>&1 | tail -20`. It fails: fix it (at most 2 attempts), else
      `git reset --hard R` and reject the branch.
   4. `python3 S/regress.py check --quick` (see `SKILL.md`, "Long commands"). `REGRESS: FAIL`: read
      `scratch/regress/last.md` and fix it (at most 2 attempts), else `git reset --hard R` and
      reject the branch.
2. Final mode with `finish: yes` only, the crate-level cleanup:
   1. `python3 S/qscan.py scan --out scratch/qscan_mid`. Its summary lists the blanket crate-level
      allows and the boilerplate `# Safety` texts (texts that more than 3 functions share).
   2. Remove each crate-level or module-level `#![allow(...)]` entry that is not one of
      `non_snake_case`, `non_camel_case_types`, `non_upper_case_globals`, `dead_code`. Fix the
      warnings that this shows when the fix is mechanical (unused variables, unused `mut`,
      unneeded parens, unneeded `unsafe` blocks): `python3 S/qscan.py autofix --commit --lints
      unused_unsafe,unused_mut,unused_parens`, then the rest by hand. Leave a warning whose fix
      is not mechanical.
   3. Rewrite each boilerplate `# Safety` text for its function: the real preconditions of that
      function's pointer parameters.
   4. `cargo build --release`, `python3 S/regress.py check --quick`, and commit.
3. `python3 S/regress.py check`: the quick check plus a short fuzz campaign per property.
   `REGRESS: FAIL` (a fuzz property): find the change that caused it with
   `python3 S/regress.py check --fuzz-filter '<Suite.Property>'` and fix it, or revert that merge
   (`git revert -m 1 --no-edit <merge sha>`) and reject the branch. Then run the check again.
4. `python3 S/qscan.py scan --out <the after scan>` (with `finish: yes`, the after scan is
   `shared/quality/final`) and
   `python3 S/qscan.py compare <the before scan> <the after scan> > scratch/compare.txt`.
   PROBLEMS: find the branch that caused each one (`qscan.py fn` shows the function) and fix it
   (at most 2 attempts) or revert its merge. Measure and compare again.
5. Write `quality/INTEGRATION_<integration branch>.md` and commit it:
   - a table: branch | merged or rejected | reason | fixes made;
   - the compare table and its PROBLEMS line (copy `scratch/compare.txt`);
   - the last 8 lines of `scratch/regress/last.md`.
   With `finish: yes`, also write `quality/FINAL.md`: the output of
   `python3 S/qscan.py compare shared/quality/baseline shared/quality/final`, the last 10 lines of
   `scratch/regress/last.md`, the allows you removed, and the first table of each section of
   `python3 S/qscan.py roots --scan shared/quality/final`.
   The last gate run must be after your last code change.

## Reply

At most 20 lines: merged and rejected branches with reasons, unsafe operations and lints before
and after, the gate line, and anything the main agent must decide.
