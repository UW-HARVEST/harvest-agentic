# Linked list types in your subtask

Your subtask uses linked list types. Your task names their cards (in `scout/sll/types/`). Read the
cards first, then the rewriting guide `inputs/agent_skills/sll/references/rewriting.md`.

1. Each card gives the representation of its type, the views line, and the Rust types. Use them
   as they are. If another subtask defines the Rust type or the view struct, your task gives its
   module path.
2. The "Scope" section of the rewriting guide tells which types and functions get its rules. Write
   them as the section of the representation shows, and obey "Rules". Translate all other code as
   your other instructions tell.
3. With `Views: YES`, only the functions that the card lists as read-only use views, and only the
   functions that it lists as single-node take a node as a reference. Write the signature and
   the body of each of these functions as the rewriting guide shows.
4. In your report, add the `list type` line of the "Report" section of the rewriting guide for
   each type. Fill its `deviations` field as that section tells. Do not copy a line from your
   task.
