"""Variable records of a debug binary (gdb_extract.py), with the values of the arrays on the stack
from their static copies (stackcopy.py). detect, translate, dump and verify_values.py use it."""
import os

from . import gdbrun, stackcopy

NO_BUILD = 'binary not from "tab2rust.py build"'


def inventory(binary):
    inv = gdbrun.inventory(binary)
    fill(inv['records'], binary)
    return inv


def dump(binary, targets):
    recs = gdbrun.dump(binary, targets)
    fill([r for r in recs if 'error' not in r], binary)
    return recs


def fill(recs, binary):
    """Each stack record gets copy=True and the values of its static copy, or copy_error (the reason)."""
    stack = [r for r in recs if r['storage'] == 'stack']
    if not stack:
        return
    m = stackcopy.load(binary)
    if m is None:
        for r in stack:
            r['copy_error'] = NO_BUILD
        return
    targets = [dict(key=str(i), name=r['name'], func=r['func'], path=os.path.realpath(r['file']), line=r['line'])
               for i, r in enumerate(stack)]
    twins = gdbrun.dump(m['library'], targets) if m['library'] else [{'error': 'no library'}] * len(stack)
    for r, t in zip(stack, twins):
        r['copy_error'] = stackcopy.why_not(m, r, t)
        if r['copy_error'] is None:
            r['copy'] = True
            r.update({k: t[k] for k in ('nonzero', 'nonzero_chararr1', 'value', 'targets') if k in t})
