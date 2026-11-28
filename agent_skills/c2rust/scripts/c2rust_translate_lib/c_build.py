"""The CMake build of the C project, and the facts of it that the translation needs."""
import json
import os
import re
from dataclasses import dataclass
from pathlib import Path

from . import ToolError, run

JOBS = min(8, os.cpu_count() or 1)
# A library that the link line names as a file: lib<name>.so, lib<name>.so.1, lib<name>.a
LIBRARY_FILE = re.compile(r'lib([^/]+?)\.(?:so|a)(?:\.[0-9.]+)?$')
MAIN_FUNCTION = re.compile(r'\bmain\s*\(')


@dataclass
class CBuild:
    command: list             # the configure command
    compile_commands: Path
    project: str              # the name of the CMake project
    executable_units: list    # the translation units with the `main` of an executable, by file name without suffix
    link_libraries: list      # the system libraries of the link lines, by name


def configure_and_build(c_src, build_dir):
    """Makes a Release build of `c_src` in the new directory `build_dir`, with the default options
    of the project."""
    query = build_dir / '.cmake' / 'api' / 'v1' / 'query'
    query.mkdir(parents=True)
    (query / 'codemodel-v2').touch()
    configure = ['cmake', '-S', str(c_src), '-B', str(build_dir), '-DCMAKE_BUILD_TYPE=Release',
                 '-DCMAKE_POLICY_VERSION_MINIMUM=3.10', '-DCMAKE_EXPORT_COMPILE_COMMANDS=ON']
    for command in (configure, ['cmake', '--build', str(build_dir), '-j', str(JOBS)]):
        status, output = run(command)
        if status != 0:
            raise ToolError(f'The C build failed: {" ".join(command)}\n{output[-3000:]}')
    compile_commands = build_dir / 'compile_commands.json'
    if not compile_commands.is_file():
        raise ToolError(f'CMake wrote no {compile_commands}')
    project, targets = read_codemodel(build_dir)
    return CBuild(configure, compile_commands, project, executable_units(targets, c_src),
                  system_libraries(targets, build_dir))


def read_codemodel(build_dir):
    """The project name and the targets of the build, from the reply of the CMake file API."""
    reply = build_dir / '.cmake' / 'api' / 'v1' / 'reply'
    index = json.loads(sorted(reply.glob('index-*.json'))[-1].read_text())
    codemodel = json.loads((reply / index['reply']['codemodel-v2']['jsonFile']).read_text())
    configuration = codemodel['configurations'][0]
    targets = [json.loads((reply / entry['jsonFile']).read_text()) for entry in configuration['targets']]
    return configuration['projects'][0]['name'], targets


def executable_units(targets, c_src):
    """The translation units that have the `main` of an executable target. An executable that
    compiles one file gives that file. One that compiles more files gives those that have the
    text of a `main` function."""
    units = []
    for target in targets:
        if target['type'] != 'EXECUTABLE':
            continue
        sources = [c_src / source['path'] for source in target.get('sources', []) if 'compileGroupIndex' in source]
        if len(sources) > 1:
            sources = [s for s in sources if s.is_file() and MAIN_FUNCTION.search(s.read_text(errors='replace'))]
        units += [s.stem for s in sources if s.stem not in units]
    return units


def system_libraries(targets, build_dir):
    """The names of the libraries that the shared libraries and executables link and that the
    project does not build, in the order of the link lines. A link line gives such a library as
    `-lNAME` or as a file outside the build."""
    libraries = []
    for target in targets:
        if target['type'] not in ('SHARED_LIBRARY', 'MODULE_LIBRARY', 'EXECUTABLE'):
            continue
        for fragment in target.get('link', {}).get('commandFragments', []):
            text = fragment['fragment']
            if fragment['role'] != 'libraries':
                continue
            if text.startswith('-l'):
                name = text[2:]
            else:
                match = LIBRARY_FILE.search(text)
                if not match or not Path(text).is_absolute() or build_dir in Path(text).parents:
                    continue
                name = match.group(1)
            if name not in libraries:
                libraries.append(name)
    return libraries
