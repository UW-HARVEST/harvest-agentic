## Data tables (tab2rust): scout 1 only

Scout 1 does this part first, before it explores. The other scouts skip it.

tab2rust finds large C data tables and writes them as Rust with the exact compiled values. Read
`inputs/agent_skills/tab2rust/SKILL.md` and `inputs/agent_skills/tab2rust/references/tool-usage.md`.
In these files, `<SKILL_DIR>` is `inputs/agent_skills/tab2rust`.

Scout 1 may write in `shared/tab2rust/`. The translation sub-agents use the build there later.

1. Build:
   `python3 inputs/agent_skills/tab2rust/scripts/tab2rust.py build inputs/c_src --out shared/tab2rust/c-build`.
   If the build fails, add the CMake options of `inputs/c_src/CMakePresets.json` or
   `inputs/c_src/CMakeLists.txt` as `-DNAME=VALUE` and try again. If it fails again, skip the rest
   of this part and give the error in your reply.
2. Detect:
   `python3 inputs/agent_skills/tab2rust/scripts/tab2rust.py detect inputs/c_src --binary <library from step 1> --json shared/tab2rust/report.json > shared/tab2rust/report.txt`.
3. For each C file with PURE or TAINTED arrays or repetitive regions, write a note
   `scout/tab2rust/files/<path in inputs/c_src, with "/" replaced by "__">.md`.
   Write the commands of a note with the paths of this run as they are: `inputs/...` for the tool
   and the source, `shared/tab2rust/c-build` for the build, and `src/...` for `--out` and `--rust`.
   The translation sub-agents run them from the root of their checkout.
   - PURE arrays: key, lines, size, notes, and one `translate` command for all of them, with
     `--out src/<module>_tables.rs`.
   - TAINTED arrays: key, lines, and reasons.
   - Repetitive regions of `#define` or enum lines: lines, the main pattern and its line count, and
     one `probe` command that works:
     - The region lines of the report are approximate. Read the source and set `--span FILE:FIRST-LAST`
       to the complete `#define` block or the complete enum (to its closing `};`).
     - Do not add `--match` to a `--span` with lines: then probe takes all names of the lines and
       skips the names that are not values. Use `--match` only to divide the names between subtasks.
     - Choose a `--tu` file of the build that includes the names.
     - Run the command once, with `--out shared/tab2rust/probe/<note name>_<first line>.json` in
       place of `--rust`. In the note, copy the command that you ran, with `--out ...` changed to
       `--rust src/<module>_consts.rs`. Do not write the command from a template. Give the number of
       values, and the skipped names and their reasons (or the error).
   - Other repetitive regions (for example code or `case` lines): lines and the main pattern only.
   Do not list DEAD arrays. Write the array parts of the notes with a small script that reads
   `shared/tab2rust/report.json`.
4. Write `scout/tab2rust/INDEX.md`: one line per note, with the C file, the counts, and the note path.
5. When you explore, do not read the bodies of the arrays in the report.
6. At the end of each proposed subtask line, add the note paths of its files:
   `— tab2rust: scout/tab2rust/files/<...>.md`. Do not count generated tables in the output
   estimate.
7. In your reply, add a section `### tab2rust` of at most 5 lines: the build result, the counts of
   PURE arrays, TAINTED arrays and regions, and the path of `INDEX.md`.
