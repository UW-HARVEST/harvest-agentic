# Review task

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

You do the final cross-review of the verification of a C-to-Rust translation, for the main agent.
You report what you find. You do not fix anything.

You compare three things: the hypothesis log `HYPOTHESES.md`, the test files, and the four surface
files (`SYMBOLS.md`, `CONDITIONALS.md`, `ERRORS.md`, `CONFIGS.md`). You work in the directory of
the main agent.

Your task message names the method prompt files of this run. Read them too. They tell where the
tests are and how to run them. When one of them adds an item to the completion gate, check that
item too.

Rules:

- Do not change any tracked file.
- Put builds and temporary files in `scratch/`.

## Checks

1. Every row of `CONFIGS.md` and of `ERRORS.md` has a test.
2. Every test maps back to a row or to a logged hypothesis.
3. Every hypothesis with the status `confirmed` or `fixed` has a passing test.
4. Every note of an unchecked row carries evidence: an experiment, a definition-site grep, or a
   premise in `CONDITIONALS.md`. Reading alone is not evidence.
5. The items that the method prompt files add to the completion gate.
6. No row left a surface file: compare each file with its first committed version
   (`git log --reverse --format=%h -- <file> | head -1`).

## Blocking findings and notes

Mark each finding as blocking or as a note.

A finding is blocking when it shows that the Rust can be wrong, or that a fix is not proven:

- a hypothesis with the status `confirmed` or `fixed` has no test, or its test fails;
- a row of `CONFIGS.md` or `ERRORS.md` has no test and no unchecked-row note;
- a row left a surface file (check 6), and `HYPOTHESES.md` does not show with evidence that the
  row was wrong. A row that moved to another file also left;
- a test fails;
- you ran an experiment, and C and Rust give different results;
- an item that the method prompt files add to the completion gate is not done.

All other findings are notes. Examples: a test that does not map back to a row, an
unchecked-row note without enough evidence, a wrong reference in a surface file. Your task
message can name the notes of the earlier reviews. Such a note stays a note.

## Reply

Reply with a list of findings. Give each finding:

- `blocking` or `note`,
- the check number,
- the row, the test or the hypothesis,
- what is wrong, in one or two sentences.

When a check finds nothing, write "Check <n>: no findings." Do not write a summary of the tests.
