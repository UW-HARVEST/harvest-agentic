5. In the same message as the scouts, also send one list scout. It decides the Rust representation
   of the linked list types. Send it also when the C code is small, and also when you read the C
   code yourself. Never decide the representation of a list type without a card: only a card of
   the list scout decides it. Give the list scout this prompt:

   ```
   Read inputs/prompts/SLL_SCOUT.md and do the task in it.
   Context window: <sub-agent window from 0.1>. Output token limit: <sub-agent output limit from 0.1>.
   ```

   It writes `scout/sll/INDEX.md` and one card for each list type in `scout/sll/types/`, and
   replies with one line for each type. The Invariants section "Linked list types" tells how the
   plan uses the cards.
