# Rules for every sub-agent

Reader: every sub-agent of this run. Your task message, or the prompt file of your role, tells
you to read this file first.

Read these files first:

- `inputs/prompts/common.md`
- `inputs/prompts/git_subagent.md`
- `inputs/prompts/run.md`
- `inputs/prompts/runtime.md`

Read each of these files completely, before you start your task. Do not cut
a file with `head`, and do not skip a file.

- Do the task that your task message gives, and only that task. When the task message names a
  prompt file of your role, that file tells the steps and the form of your reply.
- When your task message names more prompt files, read them too.
- Do not edit the state file of the main agent (`PLAN.md`, `HYPOTHESES.md`, `QUALITY.md` or
  `CONFORM.md`).
- Reply in the form that your task asks for. Do not add a summary that the task does not ask for.
- When you cannot finish an item of your task, do not make it look finished. Name the item in
  your reply, with the reason.
