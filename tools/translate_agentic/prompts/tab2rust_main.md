# Data tables (tab2rust): the main agent

tab2rust finds large C data tables and writes them as Rust with the exact compiled values.

- The prompt of scout 1 (step 0.2) also contains this line:
  `Also read inputs/prompts/translate/tab2rust_scout.md.` Scout 1 then builds the C code, detects
  the tables, and writes `scout/tab2rust/INDEX.md` with a note for each C file. The other scouts
  do not get this line.
- Never type the values of an array that `scout/tab2rust/INDEX.md` lists. A sub-agent generates it
  with tab2rust.
- In the plan, a subtask that owns such a C file lists its note paths from `scout/tab2rust/INDEX.md`:
  `— tab2rust: scout/tab2rust/files/<...>.md`.
- The prompt of the sub-agent for such a subtask also contains this line:
  `Read inputs/prompts/translate/tab2rust.md and <the note paths>.`
- One subtask generates each table. Other subtasks can use the table, also in the same wave (for
  example, the parts of one large C file). Before you send the wave, decide the Rust name and module
  of the table. Give them to the subtask that generates the table and to each subtask that uses it.
