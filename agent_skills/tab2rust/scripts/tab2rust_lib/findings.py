"""Findings: the arrays of the text scan joined with the records of the debug binary.
detect and translate both use this module, so they give the same class to an array."""
import os
import re

from . import die, keys, lexer, preproc
from .classify import classify, item, text_notes
from .deadcode import DeadCode


def under(path, root):
    return os.path.realpath(path).startswith(os.path.realpath(root) + os.sep)


class Source:
    """All C files under root: initializer candidates with text facts, and the macros."""

    def __init__(self, root):
        self.root = os.path.abspath(root)
        files = lexer.scan_tree(self.root)
        if not files:
            die(f'no C files under {self.root}')
        self.sfs, self.by_real, self.copies = {}, {}, {}
        for p, copies in files.items():
            sf = self.sfs[os.path.relpath(p, self.root)] = lexer.SrcFile(p, os.path.relpath(p, self.root))
            self.copies[sf.rel] = [os.path.realpath(x) for x in [p] + copies]
            for x in self.copies[sf.rel]:
                self.by_real[x] = sf
        self.macros = preproc.Macros(self.sfs.values())
        self.cands = []
        for sf in self.sfs.values():
            for c in lexer.initializers(sf):
                c['facts'] = lexer.extent_facts(sf, c)
                self._bodies(sf, c)
                self.cands.append(c)

    def _bodies(self, sf, c):
        """The files that c includes inside its initializer are its body. Their items count as text items."""
        c['bodies'], c['missing'] = [], []
        for name in c['facts']['includes']:
            rel = os.path.normpath(os.path.join(os.path.dirname(sf.rel), name))
            hits = [rel] if rel in self.sfs else [r for r in self.sfs if r.endswith('/' + name) or r == name]
            if len(hits) == 1:
                c['bodies'].append(hits[0])
                c['items'] += lexer.body_items(self.sfs[hits[0]])
            else:
                c['missing'].append(name)

    def check_paths(self, files, binary):
        """Stop if no file of the debug information is under the source directory."""
        if not any(under(f, self.root) for f in files if f):
            die(f'no source file of {binary} is under {self.root}. The binary was built from other files'
                ' (another copy or another path). Give the source directory of that build, or build again.')

    def attach(self, recs):
        """c['recs']: the records of each candidate. A record matches if it has the file and a line of
        the declaration. With #line, it has the name of the #line file and the presumed line, and the
        name of the candidate. A header table has one record for each unit with other bytes."""
        by_file, by_line_name = {}, {}
        for r in recs:
            by_file.setdefault(os.path.realpath(r['file']), []).append(r)
        for c in self.cands:
            sf = self.sfs[c['file']]
            last = c['eq_line'] + (1 if c['kind'] == 'concat' else 0)
            name, lo = sf.presumed(c['decl_line'])
            if name is None:
                pool, hi = [r for x in self.copies[sf.rel] for r in by_file.get(x, [])], last
            else:
                if name not in by_line_name:
                    by_line_name[name] = [r for r in recs if lexer.is_line_file(r['file'], name)]
                pool, hi = by_line_name[name], sf.presumed(last)[1]
            hits = [r for r in pool if lo <= r['line'] <= hi]
            named = [r for r in hits if r['name'] in re.findall(r'\w+', c['name'])]
            c['recs'] = named or hits  # without a name match: a macro renamed the variable

    def not_in_text(self, r):
        """Why the text scan did not find the initializer of record r."""
        if not os.path.exists(r['file']):
            return f'{os.path.basename(r["file"])} does not exist (#line or a generated file)'
        sf = self.by_real.get(os.path.realpath(r['file']))
        if sf is None:
            return f'{r["file"]} is not under the source directory'
        if os.path.realpath(r['file']) != os.path.realpath(sf.path):
            return f'the file is a copy of {sf.rel}'
        m = re.search(r'=\s*([A-Za-z_]\w*)\s*;', sf.lines[r['line'] - 1] if r['line'] <= len(sf.lines) else '')
        if m:
            return f'the initializer is the macro {m.group(1)}'
        return 'the text scan found no initializer'

    def rel(self, path):
        return os.path.relpath(os.path.realpath(path), os.path.realpath(self.root)) if under(path, self.root) else path


def storage_of(c):
    """Storage from the text, for a candidate without a record."""
    if c['func']:
        return 'function' if re.search(r'\bstatic\b', c['head']) else 'stack'
    return 'file' if re.search(r'\bstatic\b', c['head']) else 'global'


def finding(src, c, rec, recs, dead):
    """One report row."""
    f = dict(key=None, name=c['name'] if c else rec['name'], func=rec['func'] if rec else c['func'],
             file=c['file'] if c else src.rel(rec['file']),
             lines=[c['l0'], c['l1']] if c else [rec['line'], rec['line']],
             kind=c['kind'] if c else None, text_items=c['items'] if c else None,
             binary_items=rec['leaves'] if rec else None, ctype=rec['ctype'] if rec else None,
             decl=c['decl'] if c else '', storage=rec['storage'] if rec else storage_of(c),
             units=[src.rel(u) for u in rec['units']] if rec else [])
    if rec is None:
        f.update(cls='DEAD', reasons=[dead.reason(src.sfs[c['file']], c)], notes=text_notes(c))
        return f
    f.update(classify(c, rec, src.macros, None if c else src.not_in_text(rec)))
    f['key'], unique = keys.key_of(rec, recs)
    if not unique:
        f['cls'] = 'TAINTED'
        f['reasons'].append(item('key', f'{f["key"]} also selects another array'))
    return f


def collect(src, inv, min_items, exclude=None, only=None):
    """The array findings with min_items or more text items, in the files that exclude and only
    (regular expressions of relative paths) select: the file of the array or a .c file that compiles it. The text items include the items of body files.
    An array that is not in the text has no text items: its binary values count."""
    recs = inv['records']
    src.attach(recs)
    dead = DeadCode(inv)
    out, matched = [], set()
    for c in src.cands:
        matched.update(id(r) for r in c['recs'])
        if c['items'] < min_items:  # an LLM fails on long text, not on a large buffer
            continue
        out += [finding(src, c, r, recs, dead) for r in c['recs'] if r['is_array']]
        if not c['recs'] and '[' in c['head']:
            out.append(finding(src, c, None, recs, dead))
    for r in recs:
        if id(r) not in matched and r['is_array'] and r['leaves'] >= min_items and r['addr'] and not r['nobits']:
            out.append(finding(src, None, r, recs, dead))
    return [f for f in out if lexer.paths_wanted([f['file']] + f['units'], exclude, only)]  # also the compiled files
