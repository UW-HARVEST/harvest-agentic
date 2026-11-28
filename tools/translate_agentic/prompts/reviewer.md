# Review task

Read these files first:

- `inputs/prompts/subagent.md`
- `inputs/prompts/common.md`
- `inputs/prompts/environment.md`
- `inputs/prompts/anti_cheating.md`
- `inputs/prompts/git_subagent.md`
- `inputs/prompts/run.md`
- `inputs/prompts/runtime.md`
- `inputs/prompts/translate/completion_gate.md`
- `inputs/prompts/translate/translation_rules.md`
- `inputs/prompts/output_spec.md`
- `inputs/prompts/cargo_features.md`

Read each of these files completely, before you start your task. Do not cut
a file with `head`, and do not skip a file.

You check a finished C-to-Rust translation against its completion gate, for the main agent. You
report what you find. You do not fix anything.

`PLAN.md` holds the subtasks. You work in the directory of the main agent, on the `main` branch.

Rules:

- Do not change any tracked file.
- Put builds and temporary files in `scratch/`.
- Read the C source only where a check needs it.

## Checks

1. Build every configuration: `cargo build --release` with the default features, and each
   configuration of `inputs/prompts/cargo_features.md`.
2. When the project has a library, build the C library out of tree
   (`cmake -S inputs/c_src -B scratch/<build name>`). Compare the `nm -D` output of the C library
   and the Rust library. List each symbol that only one side has.
3. In all of `src/`, search with `grep` for each stub form of "No stubs" in
   `inputs/prompts/translate/translation_rules.md`. Then, for each exported
   function, read the Rust body and the C function. Check that the Rust body does what the C
   function does. A body that has the correct name and signature but does less work than the C
   function is a finding.
4. Run the smoke test of the completion gate.
5. For each subtask that `PLAN.md` marks `[x]`, check that its code is in the files on `main`. The
   code must not be only on the branch of the subtask. A merge must not have replaced it with an
   older version or a placeholder. Use `git log` and `git diff <branch> main -- <file>` when a file
   looks incomplete.

## Reply

Reply with a list of findings. Give each finding:

- the check number,
- the file and the function name (or the symbol, or the subtask),
- what is wrong, in one or two sentences,
- the C function or C file that the Rust code must match.

When a check finds nothing, write "Check <n>: no findings." Do not write a summary of the code.
