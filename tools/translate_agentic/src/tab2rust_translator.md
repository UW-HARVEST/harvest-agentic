# Data tables in your subtask

Your subtask owns C data tables. Use the tab2rust skill in `{WORK_DIR}/agent_skills/tab2rust/`.
In its `SKILL.md`, obey the "Array classes" section and steps 4 to 9 of the "Recommended workflow".
Paths that start with `scout/` are under `{WORK_DIR}`.

1. Scout 1 already made the debug build and ran `detect`. Thus, skip steps 1 to 3 of the workflow in
   `SKILL.md`. The notes that your task names (in `scout/tab2rust/files/`) list your tables and give
   ready `translate` and `probe` commands. Read your notes first.
2. Run the commands of your notes in your working directory. Change only `--out` and `--rust`. Keep
   the absolute paths: the commands use the build in `{WORK_DIR}/scout/tab2rust/c-build/` and the C
   source in `{WORK_DIR}/c_src`. A git worktree has no build, and its copy of `c_src` is not the
   source of the build. Run `build` or `detect` again only if a command fails. Then write the new
   output in `{WORK_DIR}/scout/tab2rust/<your subtask>/`, and tell this in your report.
3. In your report, list the arrays that you generated and their output files.
