### Linked list types
- `scout/sll/INDEX.md` lists the linked list types of the C code, with one card for each type in
  `scout/sll/types/`. A card gives the Rust representation of its type. All subtasks use that
  representation.
- Only the list scout makes `scout/sll/INDEX.md` and the cards, and only a card decides the
  representation of a list type. Never decide it without a card, also when the C code is small.
  If `scout/sll/INDEX.md` does not exist when you write or read this plan, send the list scout
  first: a sub-agent with the prompt `Read inputs/prompts/SLL_SCOUT.md and do the task in it.`
- This section applies to the types with the representation OWNED-SEQUENCE, OWNED-CHAIN or
  BORROWED, and to the C-LAYOUT types with `Views: YES`. It does not apply to a C-LAYOUT type with
  `Views: NO` or to a type without a card: translate them as the other sections tell. It also
  does not apply when `scout/sll/INDEX.md` has the line `No list types.` or a line that starts
  with `Error:`.
- In an `extern "C"` function, a parameter `Option<&T>` or `Option<&mut T>` has the same ABI as
  the C pointer to `T`, and NULL is `None`. Thus a reference parameter does not change the C ABI
  of an export.
- An exported function of these types takes a reference where C has a pointer to a node. This is
  the only exception to the exact C signatures of the C ABI section:
  - A function of a BORROWED type takes `Option<&Node>`.
  - A read-only function of a C-LAYOUT type with `Views: YES` takes `Option<&View>`, with the view
    struct of the card.
  - A single-node function of a C-LAYOUT type with `Views: YES` takes `Option<&mut Node>`, with the
    node struct.
- Before the first wave, copy the line of each type from `scout/sll/INDEX.md` into "Notes for
  future-me".
- A subtask whose files or functions use a type of this section lists its cards:
  `— lists: scout/sll/types/<...>.md`. The prompt of its sub-agent contains this line:
  `Read inputs/prompts/SLL_TRANSLATOR.md and <the card paths>.`
  Do not write an example `list type` line in the prompt: the sub-agent writes the line from its
  own work.
- When you do such a subtask yourself, first read `inputs/prompts/SLL_TRANSLATOR.md` and the
  cards, and obey them as a sub-agent does. Write the `list type` line yourself.
- When a type has a representation other than C-LAYOUT, put all functions that use it in
  one subtask if they fit. The subtask that defines the Rust type runs before the subtasks that
  use it.
- For a C-LAYOUT type with `Views: YES`, the subtask that defines the node struct also defines the
  view struct next to it, from the declaration on the card. It runs before the subtasks that use
  the view struct.
- When a sub-agent reports that it cannot write a function with the representation of the card,
  change the card to C-LAYOUT with `Views: NO` and the reason, and send the subtasks that use the
  type again.
- After each wave, copy the `list type` lines of the reports into the "Result" column of
  `scout/sll/INDEX.md`.
