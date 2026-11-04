"""The GoogleTest harness of the verify stage (verify_env/): build it once, run it through the
framework's runner scripts (memory and time limits), and read per-test results."""
import os
import re
import shutil
import subprocess

RUN = re.compile(r'^\[ RUN      \] (\S+)')
DONE = re.compile(r'^\[ +(OK|FAILED|SKIPPED) +\] (\S+) \(')
FUZZ_TEST = re.compile(r'FUZZ_TEST\s*\(\s*(\w+)\s*,\s*(\w+)\s*\)')


def has_harness(crate_dir):
    return os.path.isfile(os.path.join(crate_dir, 'verify_env', 'CMakeLists.txt'))


def has_fuzz(crate_dir):
    path = os.path.join(crate_dir, 'verify_env', 'CMakeLists.txt')
    with open(path, encoding='utf-8', errors='replace') as fh:
        return 'fuzztest' in fh.read().lower()


def build(crate_dir, build_dir, fuzz_mode, log):
    """Configures and builds verify_env/ into build_dir (outside the checkout). The test binary
    does not link the Rust code (it loads it with dlopen), so one build serves every checkout."""
    src = os.path.join(crate_dir, 'verify_env')
    env = dict(os.environ, CC='clang', CXX='clang++')
    args = ['cmake', '-S', src, '-B', build_dir, '-DCMAKE_BUILD_TYPE=RelWithDebInfo']
    if fuzz_mode:
        args.append('-DFUZZTEST_FUZZING_MODE=ON')
    jobs = os.environ.get('CMAKE_BUILD_PARALLEL_LEVEL', '4')
    for step in (args, ['cmake', '--build', build_dir, '--parallel', jobs]):
        log(f"regress: {' '.join(step)}")
        proc = subprocess.run(step, capture_output=True, text=True, env=env, timeout=5400)
        if proc.returncode != 0:
            tail = '\n'.join((proc.stdout + proc.stderr).strip().splitlines()[-30:])
            raise RuntimeError(f'harness build failed ({" ".join(step[:2])}):\n{tail}')
    binary = os.path.join(build_dir, 'verification_tests')
    if not os.path.isfile(binary):
        raise RuntimeError(f'the harness build made no {binary}')
    return binary


def make_runner(runner_src, runner_dir, builds):
    """A private runner directory: the framework's run scripts next to links to the shared builds
    (build-test, build-fuzz), so that the scripts find the binaries where they expect them."""
    os.makedirs(runner_dir, exist_ok=True)
    for name in ('run_tests.sh', 'run_fuzz.sh', 'memory_scope.sh'):
        dst = os.path.join(runner_dir, name)
        shutil.copyfile(os.path.join(runner_src, name), dst)
        os.chmod(dst, 0o755)
    for name, target in builds.items():
        link = os.path.join(runner_dir, name)
        if os.path.islink(link) or os.path.exists(link):
            os.remove(link)
        os.symlink(target, link)


LISTED = re.compile(r'^\[\*\] Fuzz test: (\S+)$')


def fuzz_properties(crate_dir, fuzz_binary=None):
    """Suite.Property names of the FUZZ_TEST registrations. The fuzzing binary lists them
    (`--list_fuzz_tests`); without it, every text file of verify_env/ is searched (the verify
    agent can put tests in .cc, .h, .inc or other files)."""
    if fuzz_binary and os.path.isfile(fuzz_binary):
        try:
            proc = subprocess.run([fuzz_binary, '--list_fuzz_tests'], capture_output=True, text=True, timeout=120)
            listed = sorted({m.group(1) for line in (proc.stdout + proc.stderr).splitlines()
                             if (m := LISTED.match(line.strip()))})
            if listed:
                return listed
        except (OSError, subprocess.TimeoutExpired):
            pass
    props = []
    env_dir = os.path.join(crate_dir, 'verify_env')
    for dirpath, dirnames, files in os.walk(env_dir):
        dirnames[:] = [d for d in dirnames if not d.startswith('build') and d not in ('fuzz-artifacts', 'dist', 'docs')]
        for name in files:
            try:
                with open(os.path.join(dirpath, name), encoding='utf-8') as fh:
                    text = fh.read()
            except (UnicodeDecodeError, OSError):
                continue  # a binary file
            for m in FUZZ_TEST.finditer(text):
                props.append(f'{m.group(1)}.{m.group(2)}')
    return sorted(set(props))


def parse(output):
    """{test: 'PASS'|'FAIL'|'SKIP'|'CRASH'} from GoogleTest output. A test that started and did not
    finish is a crash."""
    status = {}
    running = None
    for line in output.splitlines():
        m = RUN.match(line)
        if m:
            running = m.group(1)
            continue
        m = DONE.match(line)
        if m:
            status[m.group(2)] = {'OK': 'PASS', 'FAILED': 'FAIL', 'SKIPPED': 'SKIP'}[m.group(1)]
            if m.group(2) == running:
                running = None
    if running is not None and running not in status:
        status[running] = 'CRASH'
    return status


def run_unit(runner_dir, lib_path, log, max_restarts=20, timeout_s=None):
    """Runs every unit test. After a crash, runs again without the crashed tests, so the other tests
    still get a result. Returns ({test: status}, notes)."""
    env = dict(os.environ, RUST_LIB_PATH=lib_path)
    if timeout_s:
        env['TEST_TIMEOUT_SECONDS'] = str(timeout_s)
    status = {}
    notes = []
    excluded = []
    for _ in range(max_restarts + 1):
        args = ['bash', os.path.join(runner_dir, 'run_tests.sh')]
        if excluded:
            args.append('--gtest_filter=-' + ':'.join(excluded))
        proc = subprocess.run(args, capture_output=True, text=True, env=env)
        out = proc.stdout + '\n' + proc.stderr
        got = parse(out)
        status.update(got)
        crashed = [t for t, s in got.items() if s == 'CRASH']
        if proc.returncode in (0, 1) and not crashed:
            break  # 1: some tests failed, all of them ran
        if not crashed:
            notes.append(f'run_tests.sh exited {proc.returncode} without a test result: '
                         + ' | '.join(out.strip().splitlines()[-3:]))
            break
        excluded += crashed
        log(f'regress: {", ".join(crashed)} crashed (exit {proc.returncode}); running the other tests again')
    else:
        notes.append(f'stopped after {max_restarts} crashes')
    return status, notes


def confirm(runner_dir, tests, lib_path, baseline_lib):
    """Separates real regressions from flaky tests (FUZZ_TEST properties run random inputs in unit
    mode) and from failures that the baseline library has too. Returns {test: verdict} with
    verdict 'REGRESSION', 'FLAKY' or 'PRE-EXISTING'."""
    out = {}
    for test in tests:
        def one(lib):
            env = dict(os.environ, RUST_LIB_PATH=lib)
            proc = subprocess.run(['bash', os.path.join(runner_dir, 'run_tests.sh'), f'--gtest_filter={test}'],
                                  capture_output=True, text=True, env=env)
            return parse(proc.stdout + '\n' + proc.stderr).get(test) == 'PASS'
        if one(lib_path):
            out[test] = 'FLAKY'
        elif not one(baseline_lib):
            out[test] = 'PRE-EXISTING'
        else:
            out[test] = 'REGRESSION'
    return out
