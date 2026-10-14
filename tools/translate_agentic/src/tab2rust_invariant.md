### Data tables (tab2rust)
- Never type the values of an array that `scout/tab2rust/INDEX.md` lists. A sub-agent generates it
  with tab2rust.
- In the plan, a subtask that owns such a C file lists its note paths from `scout/tab2rust/INDEX.md`:
  `— tab2rust: scout/tab2rust/files/<...>.md`.
- The prompt of the sub-agent for such a subtask contains this line:
  `Read inputs/prompts/TRANSLATOR.md and <the note paths>.`
- One subtask generates each table. Other subtasks can use the table, also in the same wave (for
  example, the parts of one large C file). Before you send the wave, decide the Rust name and module
  of the table. Give them to the subtask that generates the table and to each subtask that uses it.
