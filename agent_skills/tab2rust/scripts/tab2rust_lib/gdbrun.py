"""Run gdb_extract.py inside gdb and return its JSON result."""
import json
import os
import shutil
import subprocess
import tempfile

from . import die, keys

EXTRACT = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'gdb_extract.py')


def run(binary, args):
    if not shutil.which('gdb'):
        die('gdb not found in PATH')
    if not os.path.exists(binary):
        die(f'binary not found: {binary}')
    with tempfile.TemporaryDirectory() as td:
        out = os.path.join(td, 'out.json')
        env = dict(os.environ, T2R_ARGS=json.dumps(args), T2R_OUT=out)
        p = subprocess.run(['gdb', '-batch', '-nx', '-q', '-x', EXTRACT, binary], env=env,
                           capture_output=True, text=True, timeout=3600)
        if not os.path.exists(out):
            die('gdb failed:\n' + (p.stderr or p.stdout)[-3000:])
        with open(out) as f:
            return json.load(f)


def inventory(binary):
    """{"records", "live", "sources"}: see gdb_extract.py."""
    return run(binary, {'mode': 'inventory'})


def dump(binary, targets):
    return run(binary, {'mode': 'dump', 'targets': targets})


def parse_key(key):
    """Target of one KEY from the report: NAME, FILE:NAME or FILE:FUNCTION:NAME, optionally with @UNIT."""
    want = keys.parse(key)
    if want is None:
        die(f'bad --symbol "{key}". Use NAME, FILE:NAME or FILE:FUNCTION:NAME, optionally with @UNIT'
            ' (copy KEY from the detect report).')
    return want
