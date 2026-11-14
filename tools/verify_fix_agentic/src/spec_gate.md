- [ ] `python3 inputs/agent_skills/spec_contracts/scripts/spec_contracts.py check DOMAINS.md`
      prints `No problems found.` When it prints a line that starts with
      `Error:`, write that line into `HYPOTHESES.md`. Then this part of the
      item is met, and the part about the review still applies. The review
      sub-agent also compared `DOMAINS.md` with the test files. For each
      `never pass` row and each `unsure` row, it checked that no TEST and no
      FUZZ_TEST domain passes a value of the class. Its reply has one line
      with the result for each of these rows.
