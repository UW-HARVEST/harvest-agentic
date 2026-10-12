"""detect: report of large arrays (strict) and repetitive regions (loose)."""
import json

from . import VERSION, findings, lexer, records, repetition
from .report import CLASSES, print_report


def repetitive(src, a, wanted):
    """Repetitive regions in the wanted files. The lines of arrays with --min or more text items and
    of their body files are not regions."""
    skip = {}
    for c in src.cands:
        if c['items'] >= a.min:
            skip.setdefault(c['file'], set()).update(range(c['l0'], c['l1'] + 1))
            for b in c['bodies']:
                skip.setdefault(b, set()).update(range(1, len(src.sfs[b].lines) + 1))
    rep = []
    for rel, sf in wanted.items():
        for s in repetition.regions(sf, a.rep_threshold, a.rep_min_lines, frozenset(skip.get(rel, ()))):
            body = [k for k in range(s['l0'], s['l1'] + 1) if sf.rawlines[k - 1].strip()]
            mid = body[len(body) // 2] if body else s['l0']
            s['sample'] = [f'{k}: {sf.rawlines[k - 1].strip()[:100]}' for k in (s['l0'], mid)]
            s['line_head'], s['line_head_lines'], s['line_head_share'] = repetition.line_head(sf, s['l0'], s['l1'])
            rep.append(s)
    return sorted(rep, key=lambda s: -s['lines'] * s['redundancy'])


def run(a):
    src = findings.Source(a.src)
    inv = records.inventory(a.binary)
    src.check_paths(inv['sources'] + [r['file'] for r in inv['records']], a.binary)
    found = findings.collect(src, inv, a.min, a.exclude, a.only)
    wanted = {rel: sf for rel, sf in src.sfs.items() if lexer.paths_wanted([rel], a.exclude, a.only)}
    size = lambda f: f['binary_items'] if f['text_items'] is None else f['text_items']
    found.sort(key=lambda f: (CLASSES.index(f['cls']), -size(f)))
    report = dict(version=VERSION, src=src.root, binary=a.binary, min_items=a.min,
                  code_lines=sum(sum(1 for l in sf.lines if l.strip()) for sf in wanted.values()),
                  findings=found, repetitive=repetitive(src, a, wanted), rep_threshold=a.rep_threshold)
    if a.json:
        with open(a.json, 'w') as f:
            json.dump(report, f, indent=1)
    print_report(report, a.rep_show)
