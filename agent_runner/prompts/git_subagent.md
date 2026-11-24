# Git rules for a sub-agent

Reader: every sub-agent.

- The work is in a git repository. When your task gives you a git worktree
  (`wt/<name>`), work only inside it. Commit your steps there with short
  messages, on its branch.
- Do not merge, do not touch the `main` branch, and do not delete your
  worktree when you finish, unless your task tells you to.
- When your task gives you no worktree, you work in the directory of the agent
  that sent you. Change only the files that your task names.
- Build outputs (`target/`, `build*/`, `cbuild/`), `scratch/` and worktrees
  (`wt/`) stay untracked. Do not commit them.
