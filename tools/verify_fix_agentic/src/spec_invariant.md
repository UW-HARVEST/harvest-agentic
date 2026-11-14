### Function contracts
- This library has function contracts in `inputs/extra_info/spec/`. The skill
  is `inputs/agent_skills/spec_contracts/SKILL.md`. Read it before you plan
  the tests.
- Each prompt of a sub-agent that writes tests names
  `inputs/agent_skills/spec_contracts/SKILL.md`. It tells the sub-agent to
  read that file first and to follow it.
- That sub-agent writes the `DOMAINS.md` rows of the classes of its
  functions. `DOMAINS.md` is the fifth surface file. The review sub-agent
  that compares `HYPOTHESES.md`, the test files and the surface files also
  compares `DOMAINS.md`.
- Each `must cover` row and each `C rejects` row of `DOMAINS.md` names its
  tests, or has a note that starts with `not covered:` and gives the reason.
- Do not write example rows into the prompt of a sub-agent.
- A command of the skill can print a line that starts with `Error:`. When
  you cannot correct the command, log that line as a hypothesis and continue.
