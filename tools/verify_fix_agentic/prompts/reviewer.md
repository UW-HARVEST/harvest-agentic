# Review task

Reader: the reviewer of the verification stage.

Read these files first:

- `inputs/prompts/subagent.md`
- `inputs/prompts/verify/verify_rules.md`

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

## Reply

Reply with a list of findings. Give each finding:

- the check number,
- the row, the test or the hypothesis,
- what is wrong, in one or two sentences.

When a check finds nothing, write "Check <n>: no findings." Do not write a summary of the tests.
