"""build: debug build (-g -O0) of a CMake project, and the list of binaries with debug info."""
import hashlib
import os
import subprocess

from . import die, elfinfo, stackcopy

FLAGS = '-g -O0'


def is_debug_binary(path):
    """A linked ELF file (shared library or program) with debug information."""
    return elfinfo.file_type(path) in ('DYN', 'EXEC') and '.debug_info' in elfinfo.read(path)['sections']


def is_archive(path):
    with open(path, 'rb') as f:
        return f.read(8) == b'!<arch>\n'


def run(a):
    src = os.path.abspath(a.src)
    if not os.path.exists(os.path.join(src, 'CMakeLists.txt')):
        die(f'no CMakeLists.txt in {src}. Build the C code yourself with "{FLAGS}" and pass the binary to --binary.')
    # default: a directory in the working directory, unique for each source directory
    out = os.path.abspath(a.out or f'tab2rust-build-{os.path.basename(src)}-{hashlib.sha1(src.encode()).hexdigest()[:6]}')
    configure = ['cmake', '-S', src, '-B', out, '-DCMAKE_BUILD_TYPE=Debug', f'-DCMAKE_C_FLAGS_DEBUG={FLAGS}',
                 f'-DCMAKE_C_FLAGS={FLAGS}', '-DCMAKE_EXPORT_COMPILE_COMMANDS=ON',
                 '-DCMAKE_POSITION_INDEPENDENT_CODE=ON', '-DBUILD_SHARED_LIBS=ON'] + ['-D' + d for d in a.define or []]
    for cmd in (configure, ['cmake', '--build', out, '-j', str(a.jobs)]):
        p = subprocess.run(cmd, capture_output=True, text=True)
        if p.returncode != 0:
            die(f'command failed: {" ".join(cmd)}\n' + (p.stdout + p.stderr)[-3000:])
    bins = []
    for dp, dn, fn in os.walk(out):
        dn[:] = [d for d in dn if d not in ('CMakeFiles', stackcopy.DIR)]
        for f in fn:
            p = os.path.join(dp, f)
            if is_archive(p):
                # gdb cannot read an archive: link it into a shared library
                so = p + '.tab2rust.so'
                if subprocess.run(['gcc', '-shared', '-o', so, '-Wl,--whole-archive', p, '-Wl,--no-whole-archive'],
                                  capture_output=True).returncode == 0:
                    bins.append(so)
            elif is_debug_binary(p):
                bins.append(p)
    print(f'build dir: {out}')
    if not bins:
        die('no ELF binary with debug info found after the build')
    print('binaries with debug info (use one as --binary, prefer the library):')
    for b in sorted(set(bins), key=lambda x: ('.so' not in x, x)):
        print(f'  {b}')
    copied, failed = stackcopy.make(src, out, bins, a.jobs)
    print(f'arrays on the stack: {copied} with a static copy (values for detect and translate), {failed} without')
