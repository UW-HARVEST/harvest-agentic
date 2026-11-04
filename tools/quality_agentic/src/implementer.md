# Global implementer task

You carry out one sub-task of a planned change of the data model of a Rust crate translated from
C. The plan is a proposal file (`quality/proposals/P<n>.md`) that the main agent chose; its scout
already committed the proposal's contract (new types, modules, adapters) on the proposal branch.
An integrator merges the sub-task branches into the proposal branch; only results that pass the
regression gate reach `main`. Be thorough: this is the high-return part of the run, and a failed
attempt is rejected without harm.

Your task message gives: your checkout (a git worktree on branch `p<n>-<k>`; run every command
from its root), the proposal file and your sub-task. `S` below means
`inputs/agent_skills/rust_quality/scripts`.

{RULES}

Also, for this task:

1. Read the proposal file before you start, and `inputs/agent_skills/rust_quality/GLOBAL.md` for
   its techniques. If the proposal uses the cursor model, read `CURSORS.md` and follow the
   proposal's cursor table.
2. Follow the contract. If the contract is wrong (it does not compile, or it cannot work), make the
   smallest change that works and describe it in your reply; do not invent a different design.
3. Change only what your sub-task owns, plus the call sites that its signature changes force. Keep
   old entry points as shims when other sub-tasks still use them.
4. Every `unsafe` that remains gets a `// SAFETY:` comment written for that place; every
   remaining `pub unsafe fn` gets a specific `# Safety` doc.
5. Commit in small steps on your branch, each one building. Do not merge, do not remove the
   worktree.

## Steps

1. `python3 S/qscan.py scan --out scratch/qscan_start` and read the functions of your sub-task
   (`python3 S/qscan.py fn <name> --scan scratch/qscan_start`, `sed -n 'A,Bp' FILE`).
2. Change the code in steps that each build: for example one function or one type at a time,
   leaves of the call graph first. After each step: `cargo build --release`. After every 2 or 3
   steps: `python3 S/regress.py check --quick` (see `SKILL.md`, "Long commands"). `REGRESS: FAIL`:
   read `scratch/regress/last.md`, find the step that caused it, and fix it or undo it
   (`git checkout -- <files>`, or `git revert`). Commit each passing step.
3. When the sub-task is done: `python3 S/qscan.py scan --out scratch/qscan_end`,
   `python3 S/qscan.py compare scratch/qscan_start scratch/qscan_end` (no PROBLEMS), and
   `python3 S/regress.py check --quick` (it must end with `REGRESS: OK`).

## Reply

At most 30 lines:

```
P<n> sub-task <k>: <done / partly done / not done>
Unsafe operations: A -> B (qscan compare); lints: C -> D; pseudo-safe: E -> F
What changed: <types, signatures, functions; count of call sites>
Deviations from the contract: <what and why, or none>
Left for other sub-tasks or not done: <list>
Last regress.py check: <its last line>
Commits: <number>; git status clean: yes/no
```
