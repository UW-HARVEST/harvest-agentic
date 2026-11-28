# Test task

Read these files first:

- `inputs/prompts/subagent.md`
- `inputs/prompts/common.md`
- `inputs/prompts/environment.md`
- `inputs/prompts/anti_cheating.md`
- `inputs/prompts/git_subagent.md`
- `inputs/prompts/run.md`
- `inputs/prompts/runtime.md`
- `inputs/prompts/verify/verify_rules.md`
- `inputs/prompts/output_spec.md`

Read each of these files completely, before you start your task. Do not cut
a file with `head`, and do not skip a file.

You write differential tests for a C-to-Rust translation: each test runs the C code and the Rust
code on the same inputs and compares the results. The C code is in `inputs/c_src`. The Rust
translation is in `src/`. The surface files `SYMBOLS.md`, `CONDITIONALS.md`, `ERRORS.md` and
`CONFIGS.md` are the test plan.

Your task message gives:

- your checkout,
- your part: rows of `CONFIGS.md` and `ERRORS.md`,
- the name of your test file,
- the method prompt files of this run. Read them: they tell how to build both sides, and how to
  write and run the tests.

## Steps

1. Read the rows of your part, and the C code of their entry points. Read only what you need.
2. Write the tests of your rows ("Surface rows" of `inputs/prompts/verify/verify_rules.md`), and
   the other tests that the method prompt files ask for, in your test file only. Name the row
   of each test in a comment.
3. Build and run your tests. A test that fails because C and Rust differ is a result: keep it.
   Do not change the Rust code, and do not weaken a test so that it passes.
4. Do not edit the surface files. Report the rows that you cannot test, with their evidence:
   the main agent writes the notes.
5. Commit your test file on the branch of your checkout.

## Reply

Reply with only these items:

- the number of tests that you wrote, and the rows that they cover,
- each test that fails: its name, the inputs, the C result and the Rust result,
- the rows that you could not test, each with its evidence ("none" when you tested all).
