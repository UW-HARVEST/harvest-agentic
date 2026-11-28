"""A run directory for the tests: the C source in inputs/, and the working directory as a git
repository with the links and the first commit that a run starts with."""
import os
import shutil
import subprocess
import sys
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent.parent / 'scripts'
IGNORED = 'target/\nbuild*/\nscratch/\n'


def git(workspace, *args):
    return subprocess.run(['git', *args], cwd=workspace, check=True, capture_output=True, text=True).stdout


def make(root, c_src):
    """Makes the run directory in `root` with a copy of the C source `c_src`. Returns the working directory."""
    root = Path(root).resolve()
    shutil.copytree(c_src, root / 'inputs' / 'c_src')
    workspace = root / 'workspace'
    workspace.mkdir()
    git(workspace, 'init', '-q', '-b', 'main')
    git(workspace, 'config', 'user.name', 'agent')
    git(workspace, 'config', 'user.email', 'agent@invalid')
    (workspace / '.gitignore').write_text(IGNORED)
    (workspace / 'inputs').symlink_to(root / 'inputs')
    git(workspace, 'add', '-A')
    git(workspace, 'commit', '-q', '-m', 'start of the run')
    return workspace


def run_script(workspace, name, *args, stdin=None, **variables):
    """Runs a script of the skill in the working directory, as an agent does, with the text
    `stdin` on its standard input and with more environment `variables`."""
    return subprocess.run([sys.executable, str(SCRIPTS / name), *args], cwd=workspace, capture_output=True, text=True,
                          input=stdin, env=dict(os.environ, **variables))
