# Conformance Log

This file is the working state of the main agent of the conformance stage. It holds state, not
rules. The log is append-only: the main agent adds an entry for each failing external test, and
updates its status.

## Log

Format per entry:
### C<N>: <failing test> — <one-line hypothesis>
- Status: open | fixed | refuted
- Evidence / expected vs actual:
- Fix (files/lines in src/):
- Outcome after re-running the test:
