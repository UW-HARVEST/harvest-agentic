"""`setup` records the baseline once; `check` compares a checkout with it."""
import json
import os
import re
import shutil
import time

from . import fuzz, gtest, rust

SHARED = os.path.join('shared', 'regress')
# The framework writes its current run scripts (from the verify stage) into the skill's runner/.
RUNNER_SRC = os.environ.get('REGRESS_RUNNER_DIR') or os.path.join(
    os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))), 'runner')


class GateError(Exception):
    pass


def _paths(crate):
    # The real directory: `shared` in a checkout is a link, and a worktree that recorded the
    # baseline can be removed later. Recorded paths must not go through it.
    shared = os.path.realpath(os.path.join(crate, SHARED))
    return dict(shared=shared, base=os.path.join(shared, 'baseline'), corpus=os.path.join(shared, 'corpus'),
                seeds=os.path.join(shared, 'seeds'),
                build_test=os.path.join(shared, 'build-test'), build_fuzz=os.path.join(shared, 'build-fuzz'),
                runner=os.path.join(crate, 'scratch', 'regress', 'runner'),
                last=os.path.join(crate, 'scratch', 'regress', 'last.md'))


def _write_tsv(path, status):
    with open(path, 'w', encoding='utf-8') as fh:
        for name in sorted(status):
            fh.write(f'{name}\t{status[name]}\n')


def _read_tsv(path):
    if not os.path.isfile(path):
        return {}
    with open(path, encoding='utf-8') as fh:
        return dict(line.rstrip('\n').split('\t', 1) for line in fh if '\t' in line)


def _runner(p, info):
    builds = {}
    if info.get('unit'):
        builds['build-test'] = os.path.realpath(p['build_test'])
    if info.get('fuzz'):
        builds['build-fuzz'] = os.path.realpath(p['build_fuzz'])
    if builds:
        gtest.make_runner(RUNNER_SRC, p['runner'], builds)


def setup(crate, log):
    p = _paths(crate)
    if os.path.isfile(os.path.join(p['base'], 'info.json')):
        raise GateError(f'the baseline exists already ({p["base"]}); {p["shared"]}/STATUS.md shows it. '
                        'setup runs once, before any change.')
    if os.path.isdir(p['base']):
        shutil.rmtree(p['base'])
    os.makedirs(p['base'], exist_ok=True)
    t0 = time.time()
    ok, tail = rust.build(crate)
    if not ok:
        raise GateError(f'cargo build --release fails before any change:\n{tail}')
    lib = rust.artifact(crate)
    base_lib = os.path.join(p['base'], os.path.basename(lib))
    shutil.copyfile(lib, base_lib)
    exports = rust.exports(lib) if lib.endswith('.so') else []
    with open(os.path.join(p['base'], 'exports.txt'), 'w', encoding='utf-8') as fh:
        fh.write(''.join(f'{e}\n' for e in exports))
    info = dict(lib=base_lib, exports=len(exports), unit=False, fuzz=False, props=[], cargo_tests=False, notes=[])

    if gtest.has_harness(crate):
        try:
            gtest.build(crate, p['build_test'], False, log)
            info['unit'] = True
            if gtest.has_fuzz(crate):
                gtest.build(crate, p['build_fuzz'], True, log)
                info['fuzz'] = True
        except RuntimeError as e:
            info['notes'].append(str(e).splitlines()[0])
            log(f'regress: WARNING: {e}')
    _runner(p, info)
    if info['unit']:
        status, notes = gtest.run_unit(p['runner'], base_lib, log)
        _write_tsv(os.path.join(p['base'], 'unit.tsv'), status)
        info['notes'] += notes
        info['unit_counts'] = _counts(status)
    if info['fuzz']:
        props = gtest.fuzz_properties(crate, os.path.join(p['build_fuzz'], 'verification_tests'))
        info['props'] = props
        if not props:
            info['notes'].append('WARNING: the harness uses FuzzTest but no FUZZ_TEST property was found; '
                                 'fuzz replays and campaigns will not run')
        counts = fuzz.collect_corpus(crate, props, p['corpus'], p['seeds'])
        info['corpus_files'] = counts
        replay = {}
        for prop in props:
            if prop in counts:
                ok, note = fuzz.replay(p['runner'], prop, os.path.join(p['corpus'], prop), base_lib)
                replay[prop] = 'PASS' if ok else 'FAIL'
                if not ok:
                    log(f'regress: baseline replay of {prop} fails: {note}')
        _write_tsv(os.path.join(p['base'], 'replay.tsv'), replay)
        info['replay_counts'] = _counts(replay)
    if rust.has_tests(crate):
        status, note = rust.cargo_test(crate)
        _write_tsv(os.path.join(p['base'], 'cargo_test.tsv'), status)
        info['cargo_tests'] = True
        info['cargo_test_counts'] = _counts(status)
        if note:
            info['notes'].append(note)
    info['seconds'] = round(time.time() - t0)
    with open(os.path.join(p['base'], 'info.json'), 'w', encoding='utf-8') as fh:
        json.dump(info, fh, indent=1)
    text = status_text(info)
    with open(os.path.join(p['shared'], 'STATUS.md'), 'w', encoding='utf-8') as fh:
        fh.write(text)
    return text


def _counts(status):
    out = {}
    for s in status.values():
        out[s] = out.get(s, 0) + 1
    return out


def status_text(info):
    lines = ['# Regression baseline', '',
             f"- Baseline library: {info['lib']} ({info['exports']} exported symbols)"]
    if info['unit']:
        lines.append(f"- Unit tests (verify_env, GoogleTest): {info.get('unit_counts')}")
    else:
        lines.append('- Unit tests: none (no verify_env/ harness)')
    if info['fuzz']:
        lines.append(f"- Fuzz properties: {len(info['props'])}; saved inputs per property: {info.get('corpus_files')}; "
                     f"baseline replay: {info.get('replay_counts')}")
    if info['cargo_tests']:
        lines.append(f"- cargo test: {info.get('cargo_test_counts')}")
    if not (info['unit'] or info['cargo_tests']):
        lines.append('- WARNING: this crate has no regression tests. `check` can only compare the build and the '
                     'exported symbols. Keep to the low-risk recipes.')
    for n in info.get('notes', []):
        lines.append(f'- note: {n}')
    lines.append('- Tests that fail in the baseline are not regressions. A regression is a test or input that '
                 'passes in the baseline and fails after your change.')
    return '\n'.join(lines) + '\n'


def load_info(crate):
    path = os.path.join(_paths(crate)['base'], 'info.json')
    if not os.path.isfile(path):
        raise GateError('no baseline: the main agent must run `regress.py setup` first, before any change')
    with open(path, encoding='utf-8') as fh:
        return json.load(fh)


def check(crate, log, quick=False, fuzz_seconds=10, fuzz_filter=None, skip_build=False):
    """Returns (ok, report text)."""
    p = _paths(crate)
    info = load_info(crate)
    if not os.path.isfile(info['lib']):
        # Without the baseline library, a failure cannot be compared with the baseline, and
        # every steady failure would look like a pre-existing one.
        raise GateError(f"the baseline library {info['lib']} is missing; the gate cannot judge failures. "
                        'Stop and report.')
    problems = []
    lines = ['# Regression check', '']
    t0 = time.time()
    if not skip_build:
        ok, tail = rust.build(crate)
        if not ok:
            return False, f'REGRESS: FAIL: cargo build --release fails:\n{tail}\n'
    lib = rust.artifact(crate)
    if lib.endswith('.so'):
        with open(os.path.join(p['base'], 'exports.txt'), encoding='utf-8') as fh:
            base_exports = set(fh.read().split())
        now = set(rust.exports(lib))
        gone, new = sorted(base_exports - now), sorted(now - base_exports)
        if gone or new:
            problems.append('exported symbols changed')
            lines.append(f"- EXPORTS CHANGED. Gone: {', '.join(gone[:20]) or '-'}. New: {', '.join(new[:20]) or '-'}")
        else:
            lines.append(f'- exported symbols: unchanged ({len(now)})')
    _runner(p, info)
    if info['unit']:
        base = _read_tsv(os.path.join(p['base'], 'unit.tsv'))
        status, notes = gtest.run_unit(p['runner'], lib, log)
        bad = sorted(t for t, s in base.items() if s == 'PASS' and status.get(t, 'NOT RUN') != 'PASS')
        fixed = sorted(t for t, s in status.items() if s == 'PASS' and base.get(t, 'PASS') != 'PASS')
        lines.append(f"- unit tests: {_counts(status)} (baseline {info.get('unit_counts')})")
        verdicts = {}
        if bad and len(bad) <= 10:
            # Run each one again, and with the baseline library: random (fuzz) tests can be flaky.
            verdicts = gtest.confirm(p['runner'], bad, lib, info['lib'])
            others = sorted(t for t, v in verdicts.items() if v != 'REGRESSION')
            if others:
                lines.append('- not counted (passed when run again, or fail with the baseline library too): '
                             + ', '.join(f'{t} ({verdicts[t]})' for t in others))
            bad = [t for t in bad if verdicts[t] == 'REGRESSION']
        passing = sum(1 for v in status.values() if v == 'PASS') + sum(1 for v in verdicts.values() if v == 'FLAKY')
        base_passing = sum(1 for v in base.values() if v == 'PASS')
        if passing < base_passing and not bad:
            problems.append(f'fewer unit tests pass than in the baseline ({passing} < {base_passing})')
        if bad:
            problems.append(f'{len(bad)} unit tests regressed')
            lines.append('- UNIT TEST REGRESSIONS: ' + ', '.join(f"{t} ({status.get(t, 'NOT RUN')})" for t in bad[:30]))
        if fixed:
            lines.append(f"- unit tests that fail in the baseline and pass now: {', '.join(fixed[:10])}")
        lines += [f'- note: {n}' for n in notes]
    if info['fuzz']:
        rx = re.compile(fuzz_filter) if fuzz_filter else None
        props = [x for x in info['props'] if rx is None or rx.search(x)]
        base_replay = _read_tsv(os.path.join(p['base'], 'replay.tsv'))
        replay_bad = []
        for prop in props:
            if base_replay.get(prop) != 'PASS':
                continue
            ok, note = fuzz.replay(p['runner'], prop, os.path.join(p['corpus'], prop), lib)
            if not ok:
                replay_bad.append(f'{prop} ({note})')
        lines.append(f"- replay of saved fuzz inputs: {sum(1 for x in props if base_replay.get(x) == 'PASS') - len(replay_bad)}"
                     f" properties pass, {len(replay_bad)} regressed")
        if replay_bad:
            problems.append(f'{len(replay_bad)} fuzz replays regressed')
            lines += [f'- REPLAY REGRESSION: {r}' for r in replay_bad[:20]]
        if not quick:
            results = {}
            for prop in props:
                seed = os.path.join(p['seeds'], prop)
                status, note = fuzz.campaign(p['runner'], prop, lib, fuzz_seconds, seed, info['lib'])
                results[prop] = status
                if status != 'PASS':
                    lines.append(f'- fuzz {prop}: {status}: {note}')
                    if status == 'REGRESSION':
                        problems.append(f'fuzz campaign {prop} regressed')
            lines.append(f'- fuzz campaigns ({fuzz_seconds} s each, {len(props)} properties): {_counts(results)}')
    if info['cargo_tests']:
        base = _read_tsv(os.path.join(p['base'], 'cargo_test.tsv'))
        status, note = rust.cargo_test(crate)
        bad = sorted(t for t, s in base.items() if s == 'PASS' and status.get(t) != 'PASS')
        lines.append(f'- cargo test: {_counts(status)}' + (f' ({note})' if note else ''))
        if bad:
            problems.append(f'{len(bad)} cargo tests regressed')
            lines.append('- CARGO TEST REGRESSIONS: ' + ', '.join(bad[:30]))
    if not (info['unit'] or info['cargo_tests']):
        lines.append('- WARNING: no regression tests exist for this crate; only the build and the exports were checked')
    lines.append(f'- time: {round(time.time() - t0)} s')
    lines.append('')
    lines.append('REGRESS: OK' if not problems else 'REGRESS: FAIL: ' + '; '.join(problems))
    text = '\n'.join(lines) + '\n'
    os.makedirs(os.path.dirname(p['last']), exist_ok=True)
    with open(p['last'], 'w', encoding='utf-8') as fh:
        fh.write(text)
    return not problems, text
