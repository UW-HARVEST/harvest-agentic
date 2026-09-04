- The working directory is a git repository. The `main` branch records the
  current state of the work.
- Only you commit on the `main` branch. Leave one commit with a short message
  for each significant step, so the history shows who changed what.
- Commit your current work before you create a worktree. A worktree starts
  from the last commit.
- For each sub-agent that changes files, create a worktree and a branch under
  the same short name: `git worktree add wt/<name> -b <name>`. Give the
  sub-agent the worktree path as its working directory, and copy these rules
  into the sub-agent prompt.
- A sub-agent works only inside its worktree. It commits its own steps with
  short messages on its own branch. It does not merge, it does not touch the
  `main` branch, and it does not delete its worktree when it finishes.
- When a sub-agent finishes, you merge its branch into `main`, you resolve the
  conflicts, and you remove the worktree with `git worktree remove wt/<name>`.
  Do not fast-forward merge, so that the history shows who changed what.
- A sub-agent that only reads or investigates needs no worktree.
- The framework maintains `.gitignore` for you. Build outputs (`target/`,
  `build*/`, `cbuild/`) and worktrees (`wt/`) stay untracked. Do not commit
  them.
