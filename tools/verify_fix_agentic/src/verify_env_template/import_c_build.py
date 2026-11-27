#!/usr/bin/env python3
"""Import the build settings of the C project into the test build: which C sources make up its
library, and with which compile definitions, include directories, options and link libraries.
They come from the C project's own CMake build, so a configuration (the -D options) changes them
the same way it changes the real C build.

Usage (CMakeLists.txt runs it at configure time):

  import_c_build.py C_SRC OUT_DIR C_COMPILER [-DOPTION=VALUE ...]

It configures C_SRC in OUT_DIR with the given C compiler and options (it does not build), reads
the CMake file API reply, and writes OUT_DIR/c_build.cmake, which sets:

  C_UNDER_TEST_SOURCES    the C sources of every library target (static, shared, object,
                          module), with their compile definitions, include directories and
                          compile options as source-file properties;
  C_UNDER_TEST_INCLUDE_DIRECTORIES  all include directories of these sources, for the tests;
  C_UNDER_TEST_LIBRARIES  the libraries that the project links (for example -lz -lm).

The sources of executables only (for example the file with main()) are not included. The
result is kept and reused while the options and the CMake files of C_SRC do not change.
"""

import glob
import hashlib
import json
import os
import subprocess
import sys


def fail(message):
    sys.exit("import_c_build.py: " + message)


def escape(value):
    return value.replace("\\", "\\\\").replace('"', '\\"').replace(";", "\\;")


def cmake_quote(value):
    return '"' + escape(value) + '"'


def cmake_list(values):
    return '"' + ";".join(map(escape, values)) + '"'


def stamp_of(c_src, args):
    """A key of the inputs: the arguments and the content of the CMake files of C_SRC."""
    digest = hashlib.sha256("\0".join(args).encode())
    for root, dirs, files in os.walk(c_src):
        dirs.sort()
        for name in sorted(files):
            if name == "CMakeLists.txt" or name.endswith(".cmake"):
                with open(os.path.join(root, name), "rb") as f:
                    digest.update(name.encode() + b"\0" + f.read())
    return digest.hexdigest()


def load_reply(build):
    reply = os.path.join(build, ".cmake", "api", "v1", "reply")
    index = sorted(glob.glob(os.path.join(reply, "index-*.json")))
    if not index:
        fail("CMake wrote no file-API reply in %s" % reply)
    with open(index[-1]) as f:
        objects = json.load(f)["objects"]
    codemodel = [o for o in objects if o["kind"] == "codemodel"]
    if not codemodel:
        fail("the CMake file-API reply has no codemodel")
    with open(os.path.join(reply, codemodel[0]["jsonFile"])) as f:
        model = json.load(f)
    targets = []
    for target in model["configurations"][0]["targets"]:
        with open(os.path.join(reply, target["jsonFile"])) as f:
            targets.append(json.load(f))
    return model["paths"]["source"], targets


def describe(source_dir, build, targets):
    """(groups, libraries): groups are (sources, defines, includes, options) of C code."""
    library_types = {"STATIC_LIBRARY", "SHARED_LIBRARY", "OBJECT_LIBRARY", "MODULE_LIBRARY"}
    seen, groups, libraries = set(), [], []
    for target in targets:
        for fragment in target.get("link", {}).get("commandFragments", []):
            value = fragment["fragment"].strip()
            # The project's own libraries (relative to its build directory, or in it) are left
            # out: the C reference compiles their sources instead.
            own = build in value or not (value.startswith("-") or os.path.isabs(value))
            if fragment.get("role") == "libraries" and not own and value not in libraries:
                libraries.append(value)
        if target["type"] not in library_types:
            continue
        for group in target.get("compileGroups", []):
            if group.get("language") != "C":
                continue
            sources = []
            for index in group["sourceIndexes"]:
                path = target["sources"][index]["path"]
                path = path if os.path.isabs(path) else os.path.join(source_dir, path)
                if path not in seen and os.path.exists(path):
                    seen.add(path)
                    sources.append(path)
            if sources:
                groups.append((
                    sources,
                    [d["define"] for d in group.get("defines", [])],
                    [i["path"] for i in group.get("includes", [])],
                    " ".join(f["fragment"] for f in group.get("compileCommandFragments", [])).split(),
                ))
    return groups, libraries


def main():
    if len(sys.argv) < 4:
        fail("usage: import_c_build.py C_SRC OUT_DIR C_COMPILER [-DOPTION=VALUE ...]")
    c_src, out, compiler = (os.path.abspath(sys.argv[1]), os.path.abspath(sys.argv[2]),
                            sys.argv[3])
    options = sys.argv[4:]
    result = os.path.join(out, "c_build.cmake")
    stamp_file = os.path.join(out, "c_build.stamp")
    stamp = stamp_of(c_src, [compiler] + options)
    if os.path.exists(result) and os.path.exists(stamp_file):
        with open(stamp_file) as f:
            if f.read() == stamp:
                return 0
    build = os.path.join(out, "project")
    query = os.path.join(build, ".cmake", "api", "v1", "query")
    os.makedirs(query, exist_ok=True)
    open(os.path.join(query, "codemodel-v2"), "w").close()
    # No build type: the compile options are then the project's own, not an optimization level.
    configure = ["cmake", "-S", c_src, "-B", build, "-DCMAKE_BUILD_TYPE=",
                 "-DCMAKE_C_COMPILER=" + compiler] + options
    run = subprocess.run(configure, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                         universal_newlines=True)
    if run.returncode != 0:
        fail("configuring the C project failed:\n%s\n%s" % (" ".join(configure), run.stdout[-4000:]))
    source_dir, targets = load_reply(build)
    groups, libraries = describe(source_dir, build, targets)
    if not groups:
        fail("the C project in %s has no library target with C sources" % c_src)
    lines = ["# Written by import_c_build.py from the CMake build of %s. Do not edit." % c_src,
             "set(C_UNDER_TEST_SOURCES)"]
    for sources, defines, includes, compile_options in groups:
        lines.append("list(APPEND C_UNDER_TEST_SOURCES %s)" % " ".join(map(cmake_quote, sources)))
        lines.append("set_source_files_properties(%s PROPERTIES\n  COMPILE_DEFINITIONS %s\n"
                     "  INCLUDE_DIRECTORIES %s\n  COMPILE_OPTIONS %s)"
                     % (" ".join(map(cmake_quote, sources)), cmake_list(defines),
                        cmake_list(includes), cmake_list(compile_options)))
    includes = []
    for group in groups:
        includes += [i for i in group[2] if i not in includes]
    lines.append("set(C_UNDER_TEST_INCLUDE_DIRECTORIES %s)" % cmake_list(includes))
    lines.append("set(C_UNDER_TEST_LIBRARIES %s)" % cmake_list(libraries))
    with open(result, "w") as f:
        f.write("\n".join(lines) + "\n")
    with open(stamp_file, "w") as f:
        f.write(stamp)
    print("C build: %d sources in %d groups, libraries: %s"
          % (sum(len(g[0]) for g in groups), len(groups), " ".join(libraries) or "none"))
    return 0


if __name__ == "__main__":
    sys.exit(main())
