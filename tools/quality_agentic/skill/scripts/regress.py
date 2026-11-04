#!/usr/bin/env python3
"""regress: the regression gate of the quality stage.

Subcommands:
  setup   Main agent only, once, before any change: build the crate and the verify stage's test
          harness (verify_env/) into shared/regress/, and record the baseline: per-test results,
          replay results of the saved fuzz inputs, cargo test results, the exported symbols, and a
          copy of the library.
  check   Build this checkout and compare it with the baseline. A test or a saved input that passes
          in the baseline and fails now is a regression. Without --quick, also run a short fuzz
          campaign per property (failures are checked against the baseline library).
          The fuzz part is a quick smoke check, much shorter than the verify stage's campaigns.

Run from the root of your checkout (the main workspace or your worktree). The last line of
`check` is `REGRESS: OK` or `REGRESS: FAIL: <reasons>`; the exit status is 0 or 1.
Docs: ../SKILL.md.
"""
import argparse
import os
import sys

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from regress_lib import gate  # noqa: E402


def log(msg):
    print(msg, file=sys.stderr, flush=True)


def main():
    ap = argparse.ArgumentParser(prog='regress.py', description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('--crate', default='.', help='checkout root (default: the current directory)')
    sub = ap.add_subparsers(dest='cmd', required=True)
    sub.add_parser('setup', help='record the baseline (once, before any change)')
    p = sub.add_parser('check', help='compare this checkout with the baseline')
    p.add_argument('--quick', action='store_true', help='unit tests and fuzz replays only, no new campaigns')
    p.add_argument('--fuzz-seconds', type=int, default=10, help='seconds per fuzz campaign (default 10)')
    p.add_argument('--fuzz-filter', help='regular expression: only these fuzz properties (Suite.Property)')
    p.add_argument('--skip-build', action='store_true', help='do not run cargo build --release first')
    a = ap.parse_args()
    crate = os.path.abspath(a.crate)
    try:
        if a.cmd == 'setup':
            print(gate.setup(crate, log), end='')
        else:
            ok, text = gate.check(crate, log, a.quick, a.fuzz_seconds, a.fuzz_filter, a.skip_build)
            print(text, end='')
            sys.exit(0 if ok else 1)
    except gate.GateError as e:
        sys.exit(f'regress: {e}')


if __name__ == '__main__':
    main()
