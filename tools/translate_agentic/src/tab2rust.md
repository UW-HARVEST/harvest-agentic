# Data tables in your subtask

Your subtask owns C data tables. Use the tab2rust skill in `inputs/agent_skills/tab2rust/`.
In its `SKILL.md`, obey the "Array classes" section and steps 4 to 9 of the "Recommended workflow".

1. Scout 1 already made the debug build in `shared/tab2rust/c-build/` and ran `detect`. Thus, skip
   steps 1 to 3 of the workflow in `SKILL.md`. The notes that your task names (in
   `scout/tab2rust/files/`) list your tables and give ready `translate` and `probe` commands. Read
   your notes first.
2. Run the commands of your notes from the root of your checkout. Change only `--out` and `--rust`.
   Only read `shared/tab2rust/`. If a command fails, run `build` or `detect` again into
   `scratch/<your subtask>/`, and tell this in your report.
3. In your report, list the arrays that you generated and their output files.
