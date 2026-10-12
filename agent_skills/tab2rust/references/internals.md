# tab2rust internals

Read this file only if the tool does not do what a translation needs and you
must change the tool. For normal use, read `tool-usage.md`.

## Files

| File | Job |
|---|---|
| `scripts/tab2rust.py` | Command line. It only parses arguments and calls one module. |
| `tab2rust_lib/lexer.py` | Text scan: comment and string stripping, initializer extents, text facts, `#line`. |
| `tab2rust_lib/preproc.py` | `#if` branches and the `#define` table (the macros that give several items). |
| `tab2rust_lib/repetition.py` | Loose repetition detector, and the first-token hint of a region. |
| `tab2rust_lib/gdbrun.py` | Starts gdb with `gdb_extract.py` and reads its JSON result. |
| `tab2rust_lib/gdb_extract.py` | Runs inside gdb. Lists the variables, the live lines and the source files, and decodes bytes. |
| `tab2rust_lib/keys.py` | Keys (`--symbol` values): the shortest unique key, and the records that a key selects. gdb_extract.py also uses it. |
| `tab2rust_lib/records.py` | The records of a binary for `detect`, `translate`, `dump` and `verify_values.py`: gdb_extract records, and the values of stack arrays from their static copies. |
| `tab2rust_lib/stackcopy.py` | `build` part: static copies of the stack arrays, their library and the manifest. Read part: find the manifest of a binary, and check a copy. |
| `tab2rust_lib/elfinfo.py` | ELF facts from the header and one `readelf` run: the file type, the section flags, the relocations, and the data objects of the dynamic symbol table. gdb_extract.py and `build` use it. |
| `tab2rust_lib/compdb.py` | `compile_commands.json`: the compiled files and their flags. `probe` and `stackcopy.py` use it. |
| `tab2rust_lib/findings.py` | Joins text candidates and gdb records. `detect` and `translate` both use it. |
| `tab2rust_lib/classify.py` | PURE or TAINTED, the reasons, and the notes. |
| `tab2rust_lib/deadcode.py` | The reason of a DEAD finding. |
| `tab2rust_lib/detect.py`, `report.py` | `detect` command, and the text form of its report. |
| `tab2rust_lib/emit.py` | Converts type descriptions and values to Rust text. |
| `tab2rust_lib/translate.py` | `translate` command: dump, classify, emit, write, `rustc` check. |
| `tab2rust_lib/consts.py` | `probe --rust`: Rust constants from probe values. Uses the literal functions of `emit.py`. |
| `tab2rust_lib/rustcheck.py` | Compiles a generated file alone with `rustc`. |
| `tab2rust_lib/dump.py`, `probe.py`, `build.py` | The other commands. |
| `tests/run_tests.py` | Regression test on `tests/fixture/`. `build` compiles it with its `CMakeLists.txt` (`tables.c` and `tables_unit2.c`. `tables_unused.c`, `tables_feature.c` and `tables_guard.h` are not compiled). Run it after each change. |
| `tests/verify_values.py` | Compares a Rust file with the C binary through `{:?}` output. |

## Data flow

```
C source --lexer, preproc--> candidates (file, lines, text items, facts), macros
                                   \
debug binary --gdb_extract--> records, live lines, source files --findings--> classes --> detect report
                 + static copies (records.py)  |
               translate: dump (values) --findings--> class --> emit --> Rust file --> rustc check

build: cmake --> debug binary --gdb_extract, findings--> stack arrays --stackcopy--> copies, library, manifest
```

## Why the debug binary

The compiler already resolved macros, `#if`, `#include`, implicit zero fill,
designated initializers, and constant expressions. The debug information
(DWARF, from `-g`) gives the type of every variable, and the binary holds its
bytes. `-O0` keeps unused static arrays. The program never runs.

The binary must be linked (a `.so` or a program). In an object file (`.o`),
pointers are not relocated and read as zero.

## gdb_extract.py

**Inventory.** It finds every variable with static storage, and every
array on the stack:

1. Compilation units: the gdb Python API has no list of them, but each
   defined symbol gives its unit (`symtab.static_block()`). The names come
   from the output of `info variables -n` and `info functions -n`. An extern
   declaration is not a definition (`symbols_named`).
2. File scope: the global and the static block of each unit list its
   variables. `global` or `file` storage comes from the block.
3. Function scope: a function block does not list its nested blocks. The
   script visits the line table of each symtab and calls `block_for_pc` for
   each entry. It walks up to the function block and keeps every block. A
   header has one symtab for each compilation unit, so the key is
   (file, compilation unit). A block gives the static variables and the
   arrays on the stack. A stack record has no address and no values.
   The compiler's own variables (`DW_AT_artificial`, for example
   `__PRETTY_FUNCTION__`) are not records.
4. Each record has the type description (`describe`), the address, the
   section and its ELF flags (`writable`: the W flag and not in the
   GNU_RELRO segment, `nobits`: no file contents), `exported` (a global
   record whose name is a data object of the dynamic symbol table), `storage` (`global`,
   `file`, `function`, or `stack`), the number of scalar values (`leaves`),
   the number of values that are not zero (`nonzero`), and `pointers` (the
   type has a pointer at any depth).
5. Each record has `units`: the `.c` files of its compilation units
   (`unit_files`). The gdb Python API has no compilation-unit object. But
   each unit has its own global and static block, so the tool takes the
   `.c` files of the symbols in these two blocks. A unit that includes a
   `.c` file gives two files. A unit without its own symbols gives none.
6. A header table has one copy in each unit that includes it. Copies with
   the same real path, line, name, and bytes become one record. Their
   units are merged. Copies with different bytes (or bytes that gdb cannot
   read) stay separate records. `keys.key_of` then adds `@UNIT` to their keys.
7. `live`: for each file, the lines with code (line tables) or a variable.
   `sources`: the files of `info sources`. `deadcode.py` uses both.

**Type description.** A JSON tree: `{"kind": "int", "size": 2, "signed": true,
"names": ["int16_t", "__int16_t", "short"]}`. `names` is the typedef chain,
outermost first. Kinds: `int`, `bool`, `float`, `enum` (with members),
`array` (`len`, `of`), `struct` (`fields` with `name`, `offset`, `type`),
`str` (pointer to a 1-byte type, `to`), `ptr` (`to`, shallow), `fnptr`, and
`unsupported` (`why`: union, bit-field, void pointer, long double; `n`: the
items of one initializer row, so that the text count can be compared).
`describe` has no cache. Do not add a cache with `str(type)` as the key.
Each anonymous struct prints as `struct {...}`, so two different structs
get the same key. Then one array gets the field layout of another array.

**Values.** `decode` reads the bytes of the variable in one block and
converts them with the description. A section without file contents
(`nobits`) reads as zeros. Floats are kept as bit patterns.

The dynamic loader writes some words of a shared library: the relocations
(`elfinfo.read`, `loader`). A word with a symbol relocation has 0 in the
file (for example a pointer to a global variable). A word with a
`RELATIVE` relocation holds an address in this file. Such a word is always
an address, never a number:

- A symbol relocation gives `{"sym": NAME, "off": ADDEND}`.
- Other pointers: `info symbol` on the address. An address inside a named
  variable or function gives `{"sym": NAME, "off": BYTES}`. A string literal
  has no symbol: a `str` pointer to it is read up to the NUL (`{"s": HEX}`).
  Thus a `str` pointer into a named char array is a reference, not a string.
- A number field with a relocation gets the address form. `emit.py` then
  refuses the table.

`read_bytes` marks the relocated words as not zero, for `nonzero`. A stack
record or a record with an unsupported type has no value.

## Static copies of stack arrays (stackcopy.py, records.py)

An array on the stack has no values in the binary. At the end of `build`,
`stackcopy.make` does these steps:

1. It takes the stack records of the binaries (gdb_extract inventory) and
   joins them to the text candidates (`findings.Source.attach`, the join of
   `detect`). Thus only real declarations get a copy. A record in a file that
   is not a compiled `.c` file (a header) gets no copy.
2. For each `.c` file, it writes a copy with `static ` in front of each
   declaration (at `decl_start`) and `#line 1 "ORIGINAL"` at the top. The
   declaration stays in its function, so local types, macros and constants
   still apply. The line numbers do not change.
3. It compiles the copy with the flags of `compile_commands.json`, then
   `-iquote` (the directory of the original) and `-g -O0 -fPIC`. These come
   last, so that they win over the flags of the build. If gcc gives an error in the
   lines of a declaration, that array gets the error as its reason. The tool
   removes it and compiles again. An error in other lines stops all arrays
   of the file.
4. It links all objects into `tab2rust-stack/stack.so` with
   `--allow-multiple-definition` (each copy has all functions of its file).
5. It writes `tab2rust-stack/stack.json`: the binaries of the build, the
   library, the compiled files, and the arrays that failed.

`records.fill` runs after each gdb read of a binary. `stackcopy.load` looks
for the manifest in the parent directories of the binary, and uses it only
if the manifest lists the binary. For each stack record, gdb reads the
twin in the library. The key has the real path and the line (`keys.matches`).
`stackcopy.why_not` checks the twin: it is a static, it has the same C type
and size, and each pointer target is defined in the library (a pointer to
another file has no target there). A good twin gives `copy=True`, the
values and the nonzero counts. Else the record gets `copy_error`.

## Classification (findings.py, classify.py, deadcode.py)

`findings.Source` scans all C files under the source directory. `--only`
and `--exclude` only filter the report. Text items come from
`lexer.scan_braces` (commas at brace level, not inside parentheses). The
text items of a host include the items of its body files.

**Join.** A record belongs to a candidate if it has the file and a line from
the declaration start to the `=`. `lexer.decl_start` goes back to the
previous `;`, `{`, or `}`, and skips a struct, union, or enum body. With
`#line`, the record has the presumed line (`SrcFile.presumed`) and a path
that ends with the `#line` file name (`lexer.is_line_file`: the compiler
joins the name to its working directory). If several records match, the
records with the name of the candidate win. `Source.check_paths` stops if
no file of the binary is under the source directory.

The function, the storage, and the name of a finding with a record come
from the record. The text gives them only for a DEAD finding (no record).
Then `lexer.function_bodies` finds the function: the outermost brace block
whose head is a function head (it ends with its parameter list).

**Findings.** A finding is an array with 30 (`--min`) or more text items,
with the items of its body files. The binary values do not count. A
concatenation counts its lines. A record without a candidate is a finding
with the note `not in text` if it is an array with 30 or more binary values
and its section has file contents.

**Class.**

1. No record: DEAD. `DeadCode.reason` takes the containers of the
   declaration from the outermost: the file, the `#if` branches, and the
   function. A container is live if the binary has a live line in it. The
   reason is the first container that is not live. If no unit has the file,
   `whole_file_branches` gives the `#if` branches that have all code lines
   of the file (not the include guard, `preproc.guard_lines`). The reason
   names them all, because the tool does not know which one is false.
2. A record: TAINTED if one of these is true, else PURE. All reasons are
   listed.
   - `stack`: the storage is `stack` and the record has no static copy. The
     detail is `copy_error`. With a copy, the record gets the note
     `stack copy` instead.
   - `type`: each unsupported part of the type.
   - `count`: the counts differ, and no known cause explains it.
   - `key`: no key selects only this record.

**Count check** (`classify.count_check`). The counts agree if they are
equal, or if each string fills one char array. The tool does not check a
concatenation without braces or a host with a missing body file. `#if`
lines explain more text items. For fewer text items, the first cause is a
macro from the `#define` table that gives several items (a comma outside
parentheses, or a brace, also through other macros). This gives the note
`macro items`. The second cause is zero fill: the binary has no more
nonzero values than the text has items. This gives the note `zero fill`.

**Notes** (`classify.text_notes`, `classify.classify`): `#if` (also
`#define` and `#undef` inside the initializer), `body in FILE`,
`macro items`, `zero fill`, `no braces`, `not in text`, `writable` (the
ELF flags of the section, with `pointers can change` if the record has
`pointers`), `exported` (the dynamic symbol table has the name), `stack copy` (the values come from a static
copy). A stack record has no section, so a stack copy never gets `writable`.

`translate` calls the same functions on the records that it dumps. It
refuses TAINTED.

## Emission (emit.py)

`Emitter.rtype` gives the Rust type and `Emitter.value` the Rust value for
a type description. Both use `ctx`, a path such as `NAME[]` or
`struct.field`, to find the pointer positions that hold NULL somewhere
(`find_nullable`). Those positions get `Option<...>`.

- The spec tables (`types`, `structs`, `enums`) are found by any name in the
  typedef chain (`Emitter._spec`). `Emitter.struct_spec` also accepts the
  default Rust name (`struct_row_name`) and `KEY[]`. `Emitter.item` puts
  `KEY[]` and `NAME[]` in `row_keys` for the `ctx` of the element struct.
  `find_nullable`, `rtype`, and `value` use the same struct name, so a
  renamed struct keeps its `Option` fields. `unused_structs` gives the keys
  that matched nothing.
- A struct without a spec path gets a `#[repr(C)]` definition in the output.
- A pointer becomes `&TARGET[i][j]`. `index_path` finds the indexes from the
  byte offset and the target type. The target is an array in the same
  command (`Emitter.batch`), a spec `pointers` entry, or the upper-case C
  name with a NOTE.
- Integers are printed with the bit width of the Rust type, so a changed
  type (for example `char` as `u8`) keeps the bits. `MIN` values use
  `i32::MIN` and similar, because their positive literal does not fit.
- `float_literal` finds the shortest digits that give the same bits. It
  writes the plain form if it is at most 3 characters longer than the
  exponent form. It checks the bits of the result.

## probe.py

The probe file includes the C file given to `--tu`, with its flags from
`compile_commands.json` (`compdb.py`). The `--tu` file must be a compiled
file of the build. The names come from the arguments and `--span` (lines of
a file, or the whole file), and `--match` filters them. Each name is one line of an array initializer, so a
compile error gives the line, and the name is dropped. The first pass finds
the kind of each name with `_Generic`. The second pass stores the values in
one array for each kind. It also stores the sign of each value, and the size
(`sizeof`) and sign (`(__typeof__(X))-1 < 0`) of each C type. The probe is
linked as a shared library, so string pointers are relocated. Then
`gdb_extract.py` reads the arrays.

gcc runs with `LC_ALL=C`, so the error text is stable. It also runs with
`-ftrack-macro-expansion=0`. Without it, gcc gives an error in a macro
expansion at the `#define` line in the header, and the probe line is only
in a note. Then no error has a probe line, and the probe fails.
`skip_reasons`
sorts the dropped names. The error `'NAME' undeclared` for the name itself
means "not defined in this configuration". A function-like macro gives the
same error, so `gcc -E -dM` lists the function-like macros to separate them.
All other errors mean "not a constant expression".

## repetition.py

Each token becomes a symbol: names `I`, numbers `N`, strings `S`. Keywords,
punctuation, and preprocessor directive names stay. A window of 160 tokens
is repetitive if the share of repeated 8-token sequences is at least the
threshold. Windows move by 40 tokens. Repetitive windows that touch each
other make one region. Lines of arrays (30 or more text items) are removed from
regions. `line_head` gives the most common first token of the lines in a
region (numbers and strings count as one token each), the number of lines
that start with it (the items of the main pattern), and their share. The window size was chosen on harvest-bench files. The detector
reports more regions than necessary on purpose, because an LLM reads the
result.

## How to change the tool

- **A new C type.** Add a kind in `describe` and `decode`
  (`gdb_extract.py`), a branch in `Emitter.rtype` and `Emitter.value`, and
  a case in `tests/fixture/tables.c`.
- **A new spec option.** Read it in `Emitter.__init__` or in the option
  merge in `translate.run`. Document it in `tool-usage.md`.
- **A new class rule or note.** Change `classify.py` or `deadcode.py`. Add
  a case to the fixture and to `PURE`, `TAINTED`, or `DEAD` in
  `tests/run_tests.py`.
- After each change, run `python3 <SKILL_DIR>/tests/run_tests.py`.

## Limits

- Stack arrays have values only from a static copy of `build`. A stack array
  in a header, or in a binary that `build` did not make, is TAINTED.
- No unions, bit-fields, function pointers, void pointers, or `long double`.
- A pointer must point at a whole element of another array.
- Values are for the host build (x86-64 Linux). Types such as `long` and
  `char` signedness follow the host.
- Text items are a lexical count. An unusual initializer can give the
  reason `count` although the compiled values are correct. Then use `dump`.
- A `dead branch` reason means that the binary has no code from the branch.
  A branch with only data and macros can be live but look dead.
- The `#define` table has only the macros of the scanned files. A macro
  from a system header or a generated header is not in it.
