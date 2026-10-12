"""Reason of a DEAD finding: why the build has no variable for an initializer in the text.

The candidate is inside nested containers: its file, the #if branches around its declaration,
and its function. A container is live if the binary has code or a variable in its lines. The
reason is the outermost container that is not live. For a file that no unit of the binary has,
the reason names the #if branches that have all code lines of the file, if there are some.
"""
import bisect
import os

from . import lexer, preproc
from .classify import item


def has_live(lines, first, last):
    """True if the sorted list lines has a line in first..last."""
    k = bisect.bisect_left(lines, first)
    return k < len(lines) and lines[k] <= last


def containers(c, branches):
    """The containers of c from the outermost: (kind, first, last, branch or None)."""
    line = c['decl_line']
    res = [('branch', b['first'], b['last'], b) for b in branches if b['first'] < line < b['last']]
    if c['func_lines']:
        res.append(('function', c['func_lines'][0], c['func_lines'][1], None))
    return sorted(res, key=lambda x: (x[1], -x[2]))


def whole_file_branches(sf, branches):
    """The #if branches that have all code lines of the file, from the outermost. An include guard
    is not such a branch."""
    code = [k + 1 for k, line in enumerate(sf.lines) if line.strip() and k not in sf.pp]
    guards = preproc.guard_lines(sf)
    return sorted((b for b in branches if code and b['if_line'] not in guards
                   and b['first'] < code[0] and code[-1] < b['last']), key=lambda b: b['first'])


def branch_text(b):
    cut = lambda t: t if len(t) <= 70 else t[:67] + '...'
    text = f'{cut(b["text"])} (line {b["first"]})'
    if b['first'] != b['if_line']:
        text += f' of {cut(b["if_text"])} (line {b["if_line"]})'
    return text


class DeadCode:
    """Finds the DEAD reasons for one inventory: {live: {file: lines}, sources: [files]}."""

    def __init__(self, inv):
        self.live = inv['live']
        self.sources = set(inv['sources'])
        self._line_files = {}  # name of a #line directive -> its live lines
        self._files = {}  # rel -> (live lines, branches)

    def live_lines(self, sf):
        """Sorted live lines of a file. With #line, the binary has the presumed lines (lexer.SrcFile.presumed)
        under the name of the #line directive."""
        lines = self.live.get(os.path.realpath(sf.path), [])
        if not sf.line_dirs:
            return lines
        own = set(lines)
        out = []
        for k in range(1, len(sf.lines) + 1):
            name, p = sf.presumed(k)
            if name is not None and name not in self._line_files:
                self._line_files[name] = {x for f, ls in self.live.items() if lexer.is_line_file(f, name) for x in ls}
            if p in (own if name is None else self._line_files[name]):
                out.append(k)
        return out

    def reason(self, sf, c):
        if sf.rel not in self._files:
            self._files[sf.rel] = self.live_lines(sf), preproc.branches(sf)
        lines, branches = self._files[sf.rel]
        if os.path.realpath(sf.path) not in self.sources and not lines:
            whole = whole_file_branches(sf, branches)
            if whole:
                return item('file', text='whole file under ' + ' and '.join(map(branch_text, whole)))
            return item('file', text='file not in the binary (not compiled, or compiles to nothing)')
        for kind, first, last, b in containers(c, branches):
            if has_live(lines, first, last):
                continue
            if kind == 'branch':
                return item('branch', text='dead branch: ' + branch_text(b))
            return item('function', text=f'function not emitted: {c["func"]}')
        return item('unknown', text='cause not found')
