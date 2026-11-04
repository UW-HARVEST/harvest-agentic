"""Count the operations that need `unsafe`, the way DARPA's patched rustc does, with a stock rustc.

The `unsafe` keyword of every `unsafe {` block is blanked in a copy of the crate, and the
`unsafe_op_in_unsafe_fn` lint is forced on. rustc then reports each operation that needs unsafe as
E0133 (an error in a safe function, a lint inside an `unsafe fn`). Positions do not move, so each
report maps back to the original source.
"""
import os
import re

from . import cargo, syntax

KINDS = (
    ('deref', re.compile(r'dereference of raw pointer')),
    ('call', re.compile(r'call to unsafe function')),
    ('static_mut', re.compile(r'use of mutable static')),
    ('extern_static', re.compile(r'use of extern static')),
    ('union', re.compile(r'access to union field')),
    ('asm', re.compile(r'inline assembly')),
    ('target_feature', re.compile(r'target_feature')),
)
CALLEE = re.compile(r'call to unsafe function `([^`]+)`')


def kind_of(text):
    for kind, rx in KINDS:
        if rx.search(text):
            return kind
    return 'other'


def strip_unsafe_blocks(src):
    """Blanks the keyword of each `unsafe {` block. After `..` (a range end) the keyword becomes a
    block label, so that `0..unsafe { n } {` still parses as a range."""
    data = src.encode()
    out = bytearray(data)
    blocks = 0
    for node, ctx in syntax.walk(syntax.parse(data)):
        if node.type == 'unsafe_block':
            start = node.start_byte + ctx.shift
            prev = node.prev_sibling
            out[start:start + 6] = b"'z:   " if prev is not None and prev.type in ('..', '..=') else b'      '
            blocks += 1
    return out.decode(), blocks


def measure(crate_dir, work_dir, files):
    """Returns (ops, other_errors, note). ops: list of dict(file, line, col, kind, detail)."""
    copy = os.path.join(work_dir, 'strip')
    cargo.copy_crate(crate_dir, copy)
    for rel in files:
        path = os.path.join(copy, rel)
        with open(path, encoding='utf-8', errors='replace') as fh:
            src = fh.read()
        stripped, _ = strip_unsafe_blocks(src)
        with open(path, 'w', encoding='utf-8') as fh:
            fh.write(stripped)
    _, lib, bins = cargo.metadata(copy)
    if lib is not None:
        targets = [['--lib']]
        note = 'library target'
    else:
        targets = [['--bin', b] for b in bins]
        note = 'binary targets' if bins else 'no library or binary target'
    ops = []
    other = []
    target_dir = os.path.join(work_dir, 'target-strip')
    for sel in targets:
        args = ['cargo', 'rustc', *sel, '--profile=check', '--message-format=json', '--',
                '--force-warn', 'unsafe_op_in_unsafe_fn', '--force-warn', 'deprecated_safe_2024',
                '-A', 'warnings']
        code, messages, _, stderr = cargo.run_json(args, copy, target_dir)
        for m in messages:
            mcode = (m.get('code') or {}).get('code', '')
            if mcode == 'E0133' or m.get('message', '').startswith('call to unsafe function'):
                rel, line, col = cargo.primary_location(m, copy)
                if rel is None:
                    continue
                text = m.get('message', '')
                callee = CALLEE.search(text)
                ops.append(dict(file=rel, line=line, col=col, kind=kind_of(text),
                                detail=callee.group(1) if callee else ''))
            elif m.get('level') == 'error' and not m.get('message', '').startswith('aborting due to'):
                rel, line, _ = cargo.primary_location(m, copy)
                other.append(f"{rel}:{line}: {m.get('message', '')}")
        if code != 0 and not messages and stderr.strip():
            other.append(stderr.strip().splitlines()[-1])
    return ops, other, note
