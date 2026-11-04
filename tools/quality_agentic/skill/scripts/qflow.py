#!/usr/bin/env python3
"""qflow: the scripted steps of the quality stage. Each step runs qscan.py and regress.py in a
fixed order, so that no agent has to do it by hand.

Subcommands:
  baseline   In the main checkout, once, before any change: record the regression baseline,
             measure the crate (shared/quality/baseline), apply the safe autofixes and keep those
             that pass the gate (shared/quality/autofix), write the root causes and the local
             units (shared/quality/UNITS.md), and commit quality/BASELINE.md.
  integrate  In the integration worktree of a local wave: merge each unit branch, keep it only
             when the quick gate passes and the compare finds no problem, run the full gate, and
             commit quality/INTEGRATION_<name>.md.

Both run long. The last line is `QFLOW: OK` or `QFLOW: FAIL: <reason>`; the exit status is 0 or 1.
Docs: ../SKILL.md.
"""
import argparse
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from qscan_lib import report  # noqa: E402

# The local units of a wave are planned for this many code lines each.
UNIT_LINES = 1500


class FlowError(Exception):
    pass


def log(msg):
    print(msg, flush=True)


def sh(args, check=True):
    """Runs a command in the current directory. Returns (exit status, combined output)."""
    proc = subprocess.run(args, capture_output=True, text=True)
    out = (proc.stdout + proc.stderr).strip()
    if check and proc.returncode != 0:
        raise FlowError(f"`{' '.join(args)}` exited {proc.returncode}:\n" + '\n'.join(out.splitlines()[-20:]))
    return proc.returncode, out


def git(*args, check=True):
    return sh(['git', *args], check)[1]


def qscan(*args):
    return sh([sys.executable, os.path.join(HERE, 'qscan.py'), *args])[1]


def gate(*args):
    """Runs `regress.py check`. Returns (passed, its last line)."""
    code, out = sh([sys.executable, os.path.join(HERE, 'regress.py'), 'check', *args], check=False)
    last = out.splitlines()[-1] if out else ''
    if code not in (0, 1) or not last.startswith('REGRESS:'):
        raise FlowError(f'regress.py check did not finish:\n' + '\n'.join(out.splitlines()[-20:]))
    log(f"  gate {' '.join(args) or '(full)'}: {last}")
    return code == 0, last


def scan(out_dir):
    qscan('scan', '--out', out_dir)
    return report.load(out_dir)


def compare(before_dir, after_dir):
    return report.compare(report.load(before_dir), report.load(after_dir))


def tidy_build_files():
    """`regress.py setup` builds the harness, which can change tracked build files. Restore the
    harness install directory (the built binaries already link it) and commit a changed
    Cargo.lock, so that the work starts from a clean `git status`."""
    if git('status', '--porcelain', '--', 'verify_env/dist'):
        git('checkout', '--', 'verify_env/dist', check=False)
    if git('status', '--porcelain', '--', 'Cargo.lock'):
        commit_file('Cargo.lock', 'quality: Cargo.lock of the baseline build')
    dirty = git('status', '--porcelain', '--untracked-files=no')
    if dirty:
        raise FlowError(f'the baseline build changed tracked files:\n{dirty}')


def commit_file(path, message):
    git('add', path)
    git('commit', '-q', '-m', message)


def baseline(_args):
    if os.path.exists(os.path.join('shared', 'regress', 'baseline', 'info.json')):
        raise FlowError('the baseline exists already (shared/regress/baseline); run baseline once, before any change')
    log('qflow baseline: regress.py setup (builds the crate and the test harness)')
    setup = sh([sys.executable, os.path.join(HERE, 'regress.py'), 'setup'])[1]
    tidy_build_files()
    log('qflow baseline: measure')
    before = scan('shared/quality/baseline')
    start = git('rev-parse', 'HEAD')
    log('qflow baseline: autofix')
    qscan('autofix', '--commit')
    fixes = [line.split(' ', 1) for line in git('log', '--format=%H %s', f'{start}..HEAD').splitlines()]
    reverted = []
    ok, last = gate()
    while not ok and fixes:
        sha, subject = fixes.pop(0)  # newest first
        git('revert', '--no-edit', sha)
        reverted.append(subject)
        ok, last = gate()
    if not ok:
        raise FlowError(f'the gate fails without any autofix: {last}')
    after = scan('shared/quality/autofix')
    cmp_text, problems = report.compare(before, after)
    roots = qscan('roots', '--scan', 'shared/quality/autofix', '--out', 'shared/quality/roots_autofix.md')
    units = qscan('plan', '--scan', 'shared/quality/autofix', '--budget-lines', str(UNIT_LINES),
                  '--out', 'shared/quality/UNITS.md')
    applied = [s for _, s in fixes]
    text = '\n'.join([
        '# Baseline', '',
        '## Regression baseline', '', setup.strip(), '',
        '## Measurements (baseline -> after autofix)', '', cmp_text.strip(), '',
        '## Autofix', '',
        f"- applied: {', '.join(s.removeprefix('quality autofix: ') for s in applied) or '-'}",
        f"- reverted (the gate failed with them): {', '.join(reverted) or '-'}", '',
        '## Root causes', '', 'Full report: `shared/quality/roots_autofix.md`.', '',
        '\n'.join(roots.splitlines()[:40]), '',
        '## Units', '', units.strip(), '',
        '## Last gate', '', last, ''])
    os.makedirs('quality', exist_ok=True)
    with open('quality/BASELINE.md', 'w', encoding='utf-8') as fh:
        fh.write(text)
    commit_file('quality/BASELINE.md', 'quality: baseline')
    tb, ta = before['totals'], after['totals']
    log(f"qflow baseline: unsafe ops {tb['unsafe_ops']} -> {ta['unsafe_ops']}, lints {tb['darpa_lints']} -> "
        f"{ta['darpa_lints']}, extra lints {tb['extra_lints']} -> {ta['extra_lints']}; autofix applied "
        f"{len(applied)}, reverted {len(reverted)}; {'PROBLEMS: ' + '; '.join(problems) if problems else 'no problems'}")
    log(f'qflow baseline: units in shared/quality/UNITS.md; report quality/BASELINE.md; gate: {last}')


def integrate(a):
    branches = [b for b in a.branches.split(',') if b]
    results = []  # (branch, merged?, reason)
    merges = {}  # branch -> merge commit
    prev = a.before
    for branch in branches:
        log(f'qflow integrate: {branch}')
        r = git('rev-parse', 'HEAD')
        code, out = sh(['git', 'merge', '--no-ff', '-m', f'Merge {branch}', branch], check=False)
        if code != 0:
            git('merge', '--abort', check=False)
            git('reset', '-q', '--hard', r)
            results.append((branch, False, 'merge conflict'))
            continue
        ok, last = gate('--quick')
        if not ok:
            git('reset', '-q', '--hard', r)
            results.append((branch, False, f'gate: {last}'))
            continue
        here = os.path.join('scratch', 'qflow', branch)
        scan(here)
        _, problems = compare(prev, here)
        if problems:
            git('reset', '-q', '--hard', r)
            results.append((branch, False, 'compare: ' + '; '.join(problems)))
            continue
        merges[branch] = git('rev-parse', 'HEAD')
        results.append((branch, True, ''))
        prev = here
    ok, last = gate()
    for branch in reversed([b for b, merged, _ in results if merged]):
        if ok:
            break
        git('revert', '-m', '1', '--no-edit', merges[branch])
        results = [(b, m and b != branch, f'full gate (fuzz): {last}' if b == branch else why) for b, m, why in results]
        ok, last = gate()
    if not ok:
        raise FlowError(f'the full gate fails after all merges were reverted: {last}')
    after = scan(a.after)
    cmp_text, problems = compare(a.before, a.after)
    gate_tail = '\n'.join(open(os.path.join('scratch', 'regress', 'last.md'), encoding='utf-8').read().splitlines()[-8:])
    rows = [f"| {b} | {'merged' if m else 'rejected'} | {why or '-'} |" for b, m, why in results]
    text = '\n'.join([f'# Integration {a.name}', '', '| branch | result | reason |', '|---|---|---|', *rows, '',
                      cmp_text.strip(), '', '## Last gate', '', gate_tail, ''])
    os.makedirs('quality', exist_ok=True)
    path = f'quality/INTEGRATION_{a.name}.md'
    with open(path, 'w', encoding='utf-8') as fh:
        fh.write(text)
    commit_file(path, f'quality: integration {a.name}')
    merged = [b for b, m, _ in results if m]
    tb = report.load(a.before)['totals']
    log(f"qflow integrate: merged {', '.join(merged) or '-'}; rejected "
        f"{', '.join(f'{b} ({why})' for b, m, why in results if not m) or '-'}")
    log(f"qflow integrate: unsafe ops {tb['unsafe_ops']} -> {after['totals']['unsafe_ops']}, lints "
        f"{tb['darpa_lints']} -> {after['totals']['darpa_lints']}; "
        f"{'PROBLEMS: ' + '; '.join(problems) if problems else 'no problems'}; report {path}; gate: {last}")


def main():
    ap = argparse.ArgumentParser(prog='qflow.py', description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest='cmd', required=True)
    p = sub.add_parser('baseline', help='record the baseline, autofix, plan the units (once)')
    p.set_defaults(run=baseline)
    p = sub.add_parser('integrate', help='merge the unit branches of a local wave through the gate')
    p.add_argument('--name', required=True, help='integration branch name, for example int1')
    p.add_argument('--branches', required=True, help='comma list of branches, in merge order')
    p.add_argument('--before', required=True, help='scan of the start of the wave, for example shared/quality/autofix')
    p.add_argument('--after', required=True, help='scan to write, for example shared/quality/wave1')
    p.set_defaults(run=integrate)
    a = ap.parse_args()
    try:
        a.run(a)
    except FlowError as e:
        print(f'QFLOW: FAIL: {e}', flush=True)
        sys.exit(1)
    print('QFLOW: OK', flush=True)


if __name__ == '__main__':
    main()
