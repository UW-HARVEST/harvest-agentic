# Verification Hypotheses Log

This file is the working state of the main agent of the verification stage. It holds state, not
rules. The hypothesis log is append-only: the main agent adds an entry for each bug hypothesis,
and updates its status.

## Hypothesis log

Format per entry:
### H<N>: <one-line hypothesis>
- Status: open | confirmed | refuted | fixed
- Evidence: <how I think I know>
- Coverage: <ERRORS.md #N / CONFIGS.md #M / CONDITIONALS.md #K / test name; omit when not tied to a row or a test>
- Files/lines suspected: <file path:line>
- Action taken: <Edit/test/none yet>
- Outcome: <what happened after the action>
