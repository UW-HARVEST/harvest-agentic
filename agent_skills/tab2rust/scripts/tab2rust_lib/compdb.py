"""compile_commands.json of a build directory: the files of the build and their compiler flags."""
import json
import os
import shlex

from . import die

KEEP = ('-I', '-D', '-U', '-isystem', '-include')  # flags with a separate value
KEEP_PREFIX = KEEP + ('-std', '-f', '-m')


def entries(build_dir):
    """{real path of the source file: entry}."""
    path = os.path.join(build_dir, 'compile_commands.json')
    if not os.path.exists(path):
        die(f'{path} not found. Run "tab2rust.py build" first.')
    with open(path) as f:
        return {os.path.realpath(os.path.join(e['directory'], e['file'])): e for e in json.load(f)}


def flags(entry):
    """The -I/-D/-U/-std/-f/-m/-include/-isystem flags of one entry. Run gcc in entry['directory'],
    so that relative paths stay correct."""
    argv = entry.get('arguments') or shlex.split(entry['command'])
    out, args = [], iter(argv[1:])
    for x in args:
        if x in KEEP:
            out += [x, next(args, '')]
        elif x.startswith(KEEP_PREFIX):
            out.append(x)
    return out


def entry_for(build_dir, tu):
    """The entry of the .c file tu."""
    e = entries(build_dir).get(os.path.realpath(tu))
    if e is None:
        die(f'{tu} is not a compiled file of {build_dir}. --tu must be a .c file of the build that includes the names.')
    return e
