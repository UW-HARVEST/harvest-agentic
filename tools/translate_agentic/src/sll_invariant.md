### Linked list types
- `scout/sll/INDEX.md` lists the linked list types of the C code, with one card for each type in
  `scout/sll/types/`. A card gives the Rust representation of its type. All subtasks use that
  representation.
- This section applies to the types with the representation OWNED-SEQUENCE, OWNED-CHAIN or
  BORROWED, and to the C-LAYOUT types with `Views: YES`. It does not apply to a C-LAYOUT type with
  `Views: NO` or to a type without a card: translate them as the other sections tell. It also
  does not apply when `scout/sll/INDEX.md` has the line `No list types.` or a line that starts
  with `Error:`.
- Before the first wave, copy the line of each type from `scout/sll/INDEX.md` into "Notes for
  future-me".
- A subtask whose files or functions use a type of this section lists its cards:
  `— lists: scout/sll/types/<...>.md`. The prompt of its sub-agent contains this line:
  `Read inputs/prompts/SLL_TRANSLATOR.md and <the card paths>.`
- When a type has a representation other than C-LAYOUT, put all functions that use it in
  one subtask if they fit. The subtask that defines the Rust type runs before the subtasks that
  use it.
- For a C-LAYOUT type with `Views: YES`, the subtask that defines the node struct also defines the
  view struct next to it, from the declaration on the card. It runs before the subtasks that use
  the view struct.
- An exported function of these types takes a reference where C has a pointer to a node. The
  reference has the same ABI as the pointer. This is the only exception to the exact C signatures
  of the C ABI section:
  - A function of a BORROWED type takes `Option<&Node>`.
  - A read-only function of a C-LAYOUT type with `Views: YES` takes `Option<&View>`, with the view
    struct of the card.
  - A single-node function of a C-LAYOUT type with `Views: YES` takes `Option<&mut Node>`, with the
    node struct.
- When a sub-agent reports that it cannot write a function with the representation of the card,
  change the card to C-LAYOUT with `Views: NO` and the reason, and send the subtasks that use the
  type again.
- After each wave, copy the `list type` lines of the sub-agent reports into the "Result" column of
  `scout/sll/INDEX.md`.
