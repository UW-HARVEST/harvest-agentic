"""rustc check of a generated Rust file."""
import os
import re
import shutil
import subprocess
import tempfile


def check_rust(path):
    """Compile the file alone with rustc and print the result. Returns False on a compile error."""
    with open(path) as f:
        if re.search(r'\b(crate|super|self)::', f.read()):
            print('  CHECK   skipped: the file uses crate paths. Build your crate to check it.')
            return True
    if not shutil.which('rustc'):
        print('  CHECK   skipped: rustc not found')
        return True
    with tempfile.TemporaryDirectory() as td:
        p = subprocess.run(['rustc', '--edition', '2021', '--crate-type', 'lib', '--emit=metadata',
                            '-o', os.path.join(td, 'x.rmeta'), os.path.abspath(path)],
                           capture_output=True, text=True, cwd=td)
    if p.returncode == 0:
        print('  CHECK   rustc: the file compiles on its own')
        return True
    print('  CHECK   rustc FAILED. First errors:')
    print('\n'.join('          ' + l for l in p.stderr.split('\n')[:15]))
    return False
