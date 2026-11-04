"""Divide the work of a scan into units for worker sub-agents.

A unit owns whole functions: one file, a line range of a large file, or several small files. Two
units of the same file never run in the same wave, so that their git merges stay simple.
"""


def units(scan, budget_lines=1500, min_score=3.0):
    """Returns a list of units: dict(files=[(file, first, last or None)], score, ops, lints, lines)."""
    by_file = {}
    for r in scan['rows']:
        by_file.setdefault(r['file'], []).append(r)
    out = []
    small = []
    for rel, rows in sorted(by_file.items()):
        rows.sort(key=lambda r: r['first_line'])
        file_score = sum(r['score'] for r in rows)
        if file_score < min_score:
            continue
        code = scan['totals']['per_file'].get(rel, {}).get('code_lines', 0)
        if code <= budget_lines:
            small.append(dict(files=[(rel, None, None)], score=file_score, ops=sum(r['ops'] for r in rows),
                              lints=sum(r['lints'] for r in rows), lines=code))
            continue
        # Split a large file at function boundaries into ranges of about budget_lines.
        cur = []
        cur_lines = 0
        for r in rows:
            n = r['last_line'] - r['first_line'] + 1
            if cur and cur_lines + n > budget_lines:
                out.append(_range_unit(rel, cur))
                cur, cur_lines = [], 0
            cur.append(r)
            cur_lines += n
        if cur:
            out.append(_range_unit(rel, cur))
    # Pack small files together up to the budget.
    small.sort(key=lambda u: -u['lines'])
    packs = []
    for u in small:
        for p in packs:
            if p['lines'] + u['lines'] <= budget_lines:
                p['files'] += u['files']
                for k in ('score', 'ops', 'lints', 'lines'):
                    p[k] += u[k]
                break
        else:
            packs.append(dict(u, files=list(u['files'])))
    out += packs
    out = [u for u in out if u['score'] >= min_score]
    out.sort(key=lambda u: -u['score'])
    return out


def _range_unit(rel, rows):
    first, last = rows[0]['first_line'], rows[-1]['last_line']
    return dict(files=[(rel, first, last)], score=sum(r['score'] for r in rows), ops=sum(r['ops'] for r in rows),
                lints=sum(r['lints'] for r in rows), lines=sum(r['last_line'] - r['first_line'] + 1 for r in rows))


def waves(unit_list, parallel):
    """Assigns units to waves: at most `parallel` units per wave, and no file twice in a wave."""
    out = []
    for u in unit_list:
        files = {f for f, _, _ in u['files']}
        for w in out:
            if len(w) < parallel and not any(files & {f for f, _, _ in v['files']} for v in w):
                w.append(u)
                break
        else:
            out.append([u])
    return out


def unit_text(u):
    parts = []
    for f, a, b in u['files']:
        parts.append(f if a is None else f'{f} lines {a}-{b}')
    return '; '.join(parts)


def render(scan, budget_lines, parallel, min_score):
    ul = units(scan, budget_lines, min_score)
    ws = waves(ul, parallel)
    lines = ['# Quality units', '',
             f'Budget: about {budget_lines} code lines per unit; at most {parallel} units per wave. '
             'Each unit owns whole functions. A worker edits only the functions of its unit.', '']
    n = 0
    for wi, w in enumerate(ws, 1):
        lines.append(f'## Wave {wi}')
        lines.append('')
        for u in w:
            n += 1
            u['id'] = f'U{n}'
            lines.append(f"- [ ] U{n}: {unit_text(u)} — score {u['score']:.0f}, unsafe ops {u['ops']}, "
                         f"lints {u['lints']}, about {u['lines']} lines")
        lines.append('')
    skipped = len({r['file'] for r in scan['rows']}) - len({f for u in ul for f, _, _ in u['files']})
    if skipped:
        lines.append(f'{skipped} files have less work than the minimum score ({min_score}) and have no unit.')
    return '\n'.join(lines) + '\n'
