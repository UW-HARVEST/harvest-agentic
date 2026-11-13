# List scout task

You decide how the linked list types of the C code in `inputs/c_src` are represented in Rust. The
main agent of a C-to-Rust translation uses your cards in its plan. Other sub-agents translate.

Rules:

- Do not write Rust functions. `inputs/` is read-only.
- Write your files only in `scout/sll/`. Put temporary output in `scratch/`.
- Use `grep`, `sed -n 'A,Bp'` and `wc -l`. Read a whole file only when it is short.

Steps:

1. Read `inputs/agent_skills/sll/SKILL.md`.
2. Run the command of its "Command" section.
   - If the last line of the output is `No list types.`, write `scout/sll/INDEX.md` with that
     line, reply with that line, and stop.
   - If the command fails, write `scout/sll/INDEX.md` with the one line
     `Error: <the error message of the command>`, reply with that line, and stop.
3. For each card in `scout/sll/types/`:
   1. Find the facts of "What the analysis does not give" in the C source. Write them in the part
      "From the source" of the card, each with `file:line`.
   2. Apply the two parts of "Decision": the representation, then the views. Do the shape check
      only when the skill tells you to.
   3. Fill the part "Decision" of the card: the representation, the reason (the row number and
      the fact that decides), the views line, and the Rust types. For OWNED-SEQUENCE, OWNED-CHAIN
      and BORROWED, write the Rust declarations of the node type and of each struct that holds
      the list, as `inputs/agent_skills/sll/references/rewriting.md` shows them. For C-LAYOUT
      with `Views: YES`, write the declaration of the view struct. For C-LAYOUT with `Views: NO`,
      write `as in C`.
4. Write `scout/sll/INDEX.md`: a table with one row for each type and the columns
   `Type | C files that use it | Class | Representation | Views | Card | Result`. Leave "Result"
   empty.

Reply with one line for each type, and nothing else:

```
<C type> — <class> — <representation, and views YES or NO for C-LAYOUT> — <reason in a few words> — card: scout/sll/types/<...>.md
```
