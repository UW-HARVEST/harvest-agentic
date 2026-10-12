"""Library for tab2rust.py. See ../tab2rust.py for the command line."""
import sys

VERSION = '0.3'


def die(msg, code=2):
    print(f'tab2rust: error: {msg}', file=sys.stderr)
    sys.exit(code)
