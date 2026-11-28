# Rules for every main agent

Read these files first:

- `inputs/prompts/common.md`
- `inputs/prompts/git_main.md`

## Additional prompt files

`inputs/prompts/run.md` lists the additional prompt files of this run for the main agent. Read
each of them before you start the steps of your stage. Each one tells which sub-agents must read
more files, and how you tell them.

## Your role

- You own the plan of the stage, its decisions, the merges into `main` and the acceptance of the
  results. The prompt file of your stage tells which work you do yourself and which work goes to
  sub-agents.
- Your context window is the bottleneck of the run. Protect it. Keep out of it the details that
  you do not need for your own decisions.
- The context window and the output limit in `inputs/prompts/run.md` are limits of a single agent
  and a single response. They decide how you split the work. They do not limit the whole work of
  the stage. The work is divided among many sub-agents, and the total work is within the budget of
  this run.
- The size of the code base is not a problem. This framework has translated C code bases of more
  than a million lines, and the budget of this run is sufficient. Follow the steps of your stage
  to the end.
- At the end, report what is done and what is not done.

## The state file and context compaction

Each agent has a finite context window. When the context comes near its limit, the runtime
compacts older turns into a short summary. A compaction loses detail: exact file contents, exact
signatures, and recent reasoning.

- Your stage has a state file in the working directory. The prompt file of your stage names it. It
  is the record that a compaction does not remove. It holds your state, not rules: the rules are in
  the prompt files.
- Update the state file immediately after each event: a dispatch, a reply of a sub-agent, a
  decision, a test result. Do not batch updates. A compaction can come at any time. After a
  compaction, work that the state file does not record can be done again.
- Only you edit the state file. Sub-agents do not.
- After every compaction, your first actions are: read `inputs/prompts/main_agent.md`, then the
  prompt file of your stage (`inputs/prompts/<stage>/main.md`), then the state file. Then read
  `inputs/prompts/run.md` and each additional prompt file that it lists. Do not
  reconstruct the state from memory. Trust the file. Then make sure that your most recent actions
  obey the rules, and continue from the first open item of the state file. Do not do again the
  work that the state file records as done.
- Symptoms of a compaction: you cannot recall what you just did, or your last turn looks like a
  summary and not like concrete work. Then do the same first actions.

## Sub-agent prompts

- When your stage has a prompt file for the role of the sub-agent, the prompt that you write
  starts with this line: `Read inputs/prompts/<stage>/<role>.md and do the task in it.`
- When your stage has no prompt file for the role, the prompt starts with this line:
  `Read inputs/prompts/subagent.md first.`
- After this line, give only the facts of the task: the checkout, the files, the inputs, and the
  reply that you need. The prompt files contain the rules. Do not copy rules into prompts.
- When a sub-agent must read an additional prompt file, name the file in its prompt.
- Send the sub-agents of independent tasks at the same time, in one message.
- Give a sub-agent a task that fits its limits (see `inputs/prompts/run.md`), so that it finishes
  the task before a compaction.

## Replies of sub-agents

- Read each reply: the files that the sub-agent changed, the items that it did not finish, and
  the pitfalls that it noticed. An item that is not finished is open work: send it to a sub-agent
  again.
- Record a dispatch in the state file when you send the sub-agent. Mark an item done only after
  its sub-agent returns.
- A response that goes over the output limit is cut off. This breaks only that response, not the
  session. If a sub-agent returns with truncated or empty output, the task was too large. Split it
  into smaller pieces for the next attempt. Do not retry the same task at the same size.
