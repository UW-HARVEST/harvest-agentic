"""FuzzTest properties of the verify stage: replay the inputs that the verify agent's campaigns
saved, and run short new campaigns that start from them.

A failure is a regression only when the baseline library passes the same input: each reproducer
of a failed campaign is replayed against the saved baseline library too.
"""
import glob
import hashlib
import os
import shutil
import subprocess


def collect_corpus(crate_dir, props, dest_root, seed_root, cap=300, seed_cap=20):
    """Copies up to cap files of verify_env/fuzz-artifacts/<Prop>.*/ (the reproducers, then the
    newest corpus files) into dest_root/<Prop>/ (named by content hash; a quick replay, not the
    whole corpus of the verify stage), and the first seed_cap of them into
    seed_root/<Prop>/: a campaign runs every seed before its time budget starts, so the whole
    corpus would use up a short campaign. Returns {prop: file count}."""
    counts = {}
    art = os.path.join(crate_dir, 'verify_env', 'fuzz-artifacts')
    for prop in props:
        files = []
        for run_dir in sorted(glob.glob(os.path.join(art, prop + '.*'))):
            for sub in ('corpus', 'reproducers'):
                for dirpath, _, names in os.walk(os.path.join(run_dir, sub)):
                    files += [os.path.join(dirpath, n) for n in names]
        if not files:
            continue
        dest = os.path.join(dest_root, prop)
        os.makedirs(dest, exist_ok=True)
        # Reproducers first (each one is a failure that the verify stage fixed), then the newest.
        files.sort(key=lambda f: (os.sep + 'reproducers' + os.sep not in f, -os.path.getmtime(f)))
        seen = set()
        for path in files:
            if len(seen) >= cap:
                break
            with open(path, 'rb') as fh:
                digest = hashlib.sha1(fh.read()).hexdigest()
            if digest not in seen:
                seen.add(digest)
                shutil.copyfile(path, os.path.join(dest, digest))
                if len(seen) <= seed_cap:
                    os.makedirs(os.path.join(seed_root, prop), exist_ok=True)
                    shutil.copyfile(path, os.path.join(seed_root, prop, digest))
        counts[prop] = len(seen)
    return counts


def _run_fuzz(runner_dir, prop, lib_path, seconds, extra_env, timeout_pad=120):
    env = dict(os.environ, RUST_LIB_PATH=lib_path, FUZZ_DURATION_SECONDS=str(max(1, seconds)))
    env.update(extra_env)
    try:
        proc = subprocess.run(['bash', os.path.join(runner_dir, 'run_fuzz.sh'), prop], capture_output=True,
                              text=True, env=env, timeout=seconds + timeout_pad + 60)
    except subprocess.TimeoutExpired:
        return 124, 'timed out'
    out = (proc.stdout + '\n' + proc.stderr).strip().splitlines()
    return proc.returncode, ' | '.join(out[-4:])


def replay(runner_dir, prop, inputs, lib_path):
    """Replays a file or a directory of inputs. Returns (passed, note)."""
    code, tail = _run_fuzz(runner_dir, prop, lib_path, 1, {'FUZZTEST_REPLAY': inputs}, timeout_pad=600)
    return code == 0, '' if code == 0 else f'exit {code}: {tail}'


def campaign(runner_dir, prop, lib_path, seconds, seed_dir, baseline_lib):
    """Runs a short campaign that starts from seed_dir. Returns (status, note) where status is
    'PASS', 'REGRESSION', 'PRE-EXISTING' or 'FLAKY' (the failure does not happen again)."""
    extra = {'FUZZTEST_TESTSUITE_IN_DIR': seed_dir} if seed_dir and os.path.isdir(seed_dir) else {}
    art_root = os.path.join(runner_dir, 'fuzz-artifacts')
    before = set(glob.glob(os.path.join(art_root, prop + '.*')))
    code, tail = _run_fuzz(runner_dir, prop, lib_path, seconds, extra)
    if code == 0:
        return 'PASS', ''
    new_dirs = sorted(set(glob.glob(os.path.join(art_root, prop + '.*'))) - before)
    repros = [p for d in new_dirs for p in glob.glob(os.path.join(d, 'reproducers', '*'))]
    if repros:
        reproduced = False
        for path in repros:
            ok_new, note = replay(runner_dir, prop, path, lib_path)
            if ok_new:
                continue
            reproduced = True
            ok_base, _ = replay(runner_dir, prop, path, baseline_lib)
            if ok_base:
                return 'REGRESSION', f'reproducer {path} fails now and passes with the baseline library ({note})'
        if not reproduced:
            return 'FLAKY', f'campaign failed (exit {code}), but its reproducers pass when replayed'
        return 'PRE-EXISTING', f'campaign failed (exit {code}); its reproducers fail with the baseline library too'
    # No reproducer (time-out, memory limit). Such failures can be flaky: run the campaign again,
    # and against the baseline library.
    again, _ = _run_fuzz(runner_dir, prop, lib_path, seconds, extra)
    if again == 0:
        return 'FLAKY', f'campaign failed (exit {code}) without a reproducer and passed when run again'
    base_code, _ = _run_fuzz(runner_dir, prop, baseline_lib, seconds, extra)
    if base_code != 0:
        return 'PRE-EXISTING', f'campaign failed (exit {code}) without a reproducer; the baseline library fails too'
    return 'REGRESSION', f'campaign failed twice (exit {code}, {again}) without a reproducer; the baseline library passes: {tail}'
