# Quality Plan

The framework wrote this file. Keep the `Invariants` section as it is; fill in the other sections.

## Invariants

### AFTER ANY COMPACTION: `cat QUALITY.md` is your FIRST action.

### My role
- I decide and audit; sub-agents build, test, refactor and fix. I do not run cargo, `regress.py`,
  `qflow.py` or `qscan.py scan`, and I do not edit `src/`. I may read files, run `git` (worktrees,
  merges of integration branches into `main`, log, diff --stat), `qscan.py roots`/`compare`/`file`
  on existing scans, and `tail`.
- I accept a result only with evidence: its report file, the last line of its gate report
  (`<checkout>/scratch/regress/last.md` ends with `REGRESS: OK`), and a `qscan.py compare` without
  PROBLEMS. A sub-agent's reply alone is not evidence.
- I never merge a sub-task branch. Integrators merge sub-task branches into a proposal branch.

### Commands (S = inputs/agent_skills/rust_quality/scripts)
- Root causes: `python3 S/qscan.py roots --scan shared/quality/<scan> --out shared/quality/roots_<scan>.md`
- Compare: `python3 S/qscan.py compare shared/quality/<before> shared/quality/<after>`

{RULES}

### Branches
- `u<n>`: local unit n. `int<w>`: integration of local wave w.
- `p<n>`: proposal n (the scout's contract, then the merged sub-tasks). `p<n>-<k>`: sub-task k of
  proposal n.
- `final<r>`: integration of the accepted proposals of global round r.
- I merge only `int<w>` and `final<r>` into `main`, with `git merge --no-ff`, after I checked their
  evidence. The baseline runner commits on `main` directly.

## Baseline
(From quality/BASELINE.md.)

## Local phase
(Units by wave with checkboxes; one line per integration result.)

## Global phase
(Clusters and scouts; proposals with expected gain, risk and decision; results of each round.)

## Log
(One line per event, with numbers.)

## Notes for future-me
(Decisions, pitfalls, where you stopped and what to do next.)
