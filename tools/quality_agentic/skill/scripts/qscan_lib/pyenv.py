"""The Python environment of the Rust parser: a venv with pinned tree-sitter packages, made on
first use. All checkouts of a run use the one in `shared/.venv` (a worktree's `shared` is a link
to it); a crate without `shared/` uses `scratch/.venv`; QSCAN_VENV overrides both."""
import fcntl
import os
import subprocess
import sys

PACKAGES = ('tree-sitter==0.26.0', 'tree-sitter-rust==0.24.2')


def ensure(crate_dir):
    """Returns when tree-sitter can be imported. Else runs this program again with the venv's
    Python, and makes the venv first when it is not there (this needs network)."""
    try:
        import tree_sitter, tree_sitter_rust  # noqa: F401
        return
    except ImportError:
        pass
    shared = os.path.join(crate_dir, 'shared')
    venv = os.environ.get('QSCAN_VENV') or os.path.join(
        os.path.realpath(shared) if os.path.isdir(shared) else os.path.join(crate_dir, 'scratch'), '.venv')
    python = os.path.join(venv, 'bin', 'python')
    if os.environ.get('QSCAN_IN_VENV'):
        sys.exit(f'qscan: {python} cannot import tree_sitter; remove {venv} and run again')
    os.makedirs(os.path.dirname(venv), exist_ok=True)
    with open(venv + '.lock', 'w') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)  # another checkout can make the same venv now
        if not os.path.exists(os.path.join(venv, 'ready')):
            print(f'qscan: installing {" ".join(PACKAGES)} into {venv} (first use, needs network) ...',
                  file=sys.stderr)
            try:
                subprocess.run([sys.executable, '-m', 'venv', '--clear', venv], check=True)
                subprocess.run([python, '-m', 'pip', 'install', '-q', '--disable-pip-version-check', *PACKAGES],
                               check=True)
            except (OSError, subprocess.CalledProcessError) as e:
                sys.exit(f'qscan: could not make the venv {venv}: {e}')
            open(os.path.join(venv, 'ready'), 'w').close()
    os.environ['QSCAN_IN_VENV'] = '1'
    os.execv(python, [python, *sys.argv])
