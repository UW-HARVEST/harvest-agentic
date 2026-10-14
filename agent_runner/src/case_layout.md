### Directories of this run
All paths in this task are relative to the root of your checkout: the working
directory of the main agent, or the git worktree of a sub-agent.
- `inputs/` holds the inputs of the run: the C source `inputs/c_src/`, task
  files, skills and test suites. It is read-only. Never write, delete, move or
  change the permissions of anything in it.
- `shared/` is for large files that several agents must use but that must not
  go into git, for example a C build that a tool made once for all sub-agents.
  Git does not control who writes in it. The main agent decides, for each
  sub-agent, which files in `shared/` it may write and which it may read, and
  tells this in the sub-agent prompt. `shared/` is deleted when the run ends.
- `scratch/` is your private temporary space: C builds, test programs, large
  dumps. Git ignores it. It is deleted when the run ends.
- Put reports and anything that a later agent needs in the working directory
  (git keeps them), not in `shared/` or `scratch/`.
- `inputs` and `shared` are symbolic links that git tracks, so each git
  worktree has them. Do not delete, change or commit changes to them.
- Build the C source out of tree: `cmake -S inputs/c_src -B scratch/<build name>`.
- When you dispatch sub-agents, copy this whole "Directories of this run"
  section into every sub-agent prompt.
