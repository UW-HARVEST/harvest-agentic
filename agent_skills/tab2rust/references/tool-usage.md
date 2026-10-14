# tab2rust tool usage

`<SKILL_DIR>/scripts/tab2rust.py` is the command. `<SKILL_DIR>` is the directory
that contains SKILL.md. Write its absolute path in each command. The command works
from any working directory. It needs Python 3, `gcc`, `cmake`, `gdb`, and `readelf`.
It uses `rustc` to check its output.

## How it works (short)

1. `build` compiles the C code with debug information (`-g -O0`).
2. `gdb` reads the debug information of the library. It gets the type,
   the size, and the bytes of each array. The program does not run.
3. The tool converts these bytes to Rust literals.

The values are the values of the compiled program. The C text is only used
to check that the array is regular and to find line numbers.

## Commands

| Command | Use |
|---|---|
| `build SRC_DIR [--out DIR] [-DNAME=VALUE ...]` | Debug build of a CMake project, and static copies of the stack arrays. Prints the binaries. |
| `detect SRC_DIR --binary LIB` | Report of arrays and repetitive regions. Changes nothing. |
| `translate --src SRC_DIR --binary LIB --symbol KEY ... --out FILE.rs` | Writes Rust for the arrays. |
| `dump --binary LIB --symbol KEY [--flat]` | Prints the compiled values. Use it to check your own translations. |
| `probe --build DIR --tu FILE.c [NAME ...] [--span FILE[:FIRST-LAST]] [--match REGEX] [--rust FILE.rs]` | Gives the compiled values of macros and enum constants, as JSON or as Rust constants. |

### build

```
python3 <SKILL_DIR>/scripts/tab2rust.py build path/to/test_case --out work/c-build
```

- The directory must contain `CMakeLists.txt`.
- Without `--out`, the build directory is `tab2rust-build-<name>-<hash>` in the
  current working directory.
- The command prints the build directory and each binary that has debug
  information. Use the library (`.so`) as `--binary`.
- If the project does not use CMake, compile it yourself with `-g -O0` into
  a shared library or a program. Then give that file to `--binary`. Such a
  binary has no static copies, so all stack arrays are TAINTED.
- Use `-DNAME=VALUE` to set a CMake option, for example
  `-DSQLITE_TEST=OFF`. Repeat it for more options.
- After the build, the command makes a static copy of each array on the
  stack. It copies the `.c` file, puts `static` in front of the
  declaration, and compiles the copy with the flags of the build. The copies
  are in `<build dir>/tab2rust-stack/`. `detect`, `translate`, and `dump`
  find them from the binary path. The command prints how many arrays have a
  copy. An array in a header file gets no copy.

### detect

```
python3 <SKILL_DIR>/scripts/tab2rust.py detect path/to/test_case --binary work/c-build/libfoo.so
```

Options:

- `--min N`: minimum text items for an array. Default 30. The items of an
  included body file count too. An array that is `not in text` uses its
  binary values.
- `--rep-threshold R`: minimum redundancy for a repetitive region. Default 0.70.
- `--rep-min-lines N`: minimum lines for a repetitive region. Default 15.
- `--rep-show N`: number of repetitive regions to print. Default 25. `0`
  prints none.
- `--exclude REGEX`: do not show arrays and regions in files whose relative
  path matches. For an array, the `.c` files that compile it (the `in`
  column) also count.
- `--only REGEX`: show only arrays and regions in files whose relative path
  matches. For an array, the `.c` files that compile it also count. Thus
  `--only 'src/foo\.c$'` also shows the tables of the headers and included
  files that `foo.c` compiles.
- `--json FILE`: also write the full report as JSON.

`--binary` is required. The binary must come from the same source directory.
If no source file of the binary is under the source directory, the command
stops with an error. Then give the source directory of the build, or build again.

### translate

```
python3 <SKILL_DIR>/scripts/tab2rust.py translate --src path/to/test_case \
    --binary work/c-build/libfoo.so --symbol gear_table --symbol foo.c:lookup:table \
    --out src/tables.rs
```

- `--src DIR` (required): the C source directory. Use the same directory as
  for `detect`. `translate` uses the same rules as `detect`, so it gives the
  same class.
- `--symbol KEY` (required, repeat for more arrays): copy KEY from the
  report.
- `--out FILE.rs` (required): the output file. The tool overwrites it.
- `--name KEY=RUST_NAME` (optional, repeat): the Rust name of one array.
- `--spec FILE.json` (optional): more choices. See "Spec file" below.
- `--no-check` (optional): do not compile the output with `rustc`.

Put all arrays that point to each other in one command. Then the pointers
between them become Rust references automatically.

### dump and probe

These commands are for your own translators. See
`references/repetitive-structures.md`.

```
python3 <SKILL_DIR>/scripts/tab2rust.py dump --binary LIB --symbol KEY --flat
python3 <SKILL_DIR>/scripts/tab2rust.py probe --build BUILD_DIR --tu src/foo.c --span src/foo.h:100-300 --rust src/consts.rs
python3 <SKILL_DIR>/scripts/tab2rust.py probe --build BUILD_DIR --tu src/foo.c NAME1 NAME2
```

- `dump --flat` prints one value per line, in memory order.
- `probe` compiles the names in the context of the file given to `--tu`.
  `--tu` must be a `.c` file of the build that includes the names, not a
  header.
- `--span FILE:FIRST-LAST` takes the `#define` names and enum names from
  those lines. Do not add `--match` to get only the values: probe skips the
  names that are not values and tells why. `--span FILE` takes the names
  from the whole file. Then `--match REGEX` is necessary, for example
  `--match '^(CHAR|STR)_'`. `--match` also filters the names that you give
  as arguments. probe tells how many names `--match` did not select.
- A relative path in `--tu` and `--span` starts at the current directory.
- `--span` does not find the items of an X-macro list. Give these names as
  arguments.
- `probe` prints JSON. `--out FILE` writes the JSON to a file.
- `probe --rust FILE.rs` writes one `pub const` for each value, and does not
  print the JSON. It prints the count of each type, the first constant, and
  the last constant. Then `rustc` checks the file.
- `--int-type TYPE`: the Rust type of all integers. The default `c` gives
  the size and sign of the C type: `'A'` is `int` (`i32`),
  `((unsigned char)0x85)` is `u8`, `4000000000u` is `u32`. If a value does
  not fit TYPE, the command stops.
- `--str-type cstr|bytes|bytes_nul|str`: the Rust type of strings, as in the
  spec key `strings`. The default is `cstr`. `str` (`&str`) needs UTF-8
  strings. Otherwise the command stops with a message.
- `probe` skips some names and prints them on stderr, in three groups:

| Message | Cause |
|---|---|
| `skipped, not defined in this configuration` | The build does not define the name. Its `#define` is in an `#if` branch that is not compiled, or the `--tu` file does not include it. Do not copy it by hand from another `#if` branch. |
| `skipped, function-like macro` | The name needs arguments. |
| `skipped, not a constant expression` | The value is not a compile-time constant (for example a function name or a statement). |
| `skipped, an address, not a constant value` | The name is a char array or a pointer to one. Translate the array itself. |

## The detect report

The report has these parts.

1. A header with the source directory, the binary, and a count for each class.
2. `== PURE ==`. Arrays that the tool can translate. One line for each array:

   ```
   key | file:lines | items text/binary | C type | storage [| in FILES] [| notes: NOTES]
   ZSTD_ldm_gearTab | src/compress/zstd_ldm_geartab.h:17-104 | 256/256 | const U64 [256] | file | in src/compress/zstd_ldm.c
   _pcre2_OP_lengths_8 | src/pcre2_tables.c:67-67 | 1/173 | const uint8_t [173] | global | notes: macro items (OP_LENGTHS)
   ```

   - `key` is the value for `--symbol`. A key has one of these forms:
     `NAME`, `FILE:NAME` (when two files use the name), or
     `FILE:FUNCTION:NAME` (an array inside a function). A header table can
     have other values in each compilation unit. Then the key ends with
     `@UNIT`, for example `tables.h:tab@a.c`.
   - `items text/binary`: items in the source text and scalar values in the
     binary. A struct row with 4 fields counts as 4 values. `-` means that
     the text scan did not find the initializer.
   - `storage`: `global` (file scope, other files can use it), `file` (file
     scope, `static`), or `function NAME` (inside a function). An array on
     the stack also shows `function NAME`. It has the note `stack copy` or
     the reason `stack`.
   - `in FILES`: the `.c` files that compile the array. The report shows it
     when the array is in a header or in another included file. A header
     table in several `.c` files is one array in the C build. Translate it
     one time. `in ?` means that the tool cannot find the `.c` file.
   - `notes`: see the note table in SKILL.md. Each note has a short tag and
     sometimes a detail in parentheses.
3. `== TAINTED ==`. Arrays that the tool cannot translate. The lines have
   the same columns, and `why: REASONS` before the notes. The tool lists all
   reasons:

   | Reason | Cause |
   |---|---|
   | `stack (WHY)` | The array is on the stack (inside a function, not `static`), and it has no static copy. WHY is the cause: a compiler message such as `line 213: initializer element is not constant` (the initializer uses a variable), `not in a compiled .c file` (a header), `pointers into other files`, or `binary not from "tab2rust.py build"`. |
   | `type (WHAT)` | The C type has a union, a function pointer, a void pointer, a bit-field, or `long double`. |
   | `count (...)` | The text and the binary have different counts, and no note explains it. Read the source. |
   | `key (...)` | No key selects only this array. |

4. `== DEAD ==`. Arrays that are not in this build. One short line each:

   ```
   name | file:lines | text items | reason [| notes: NOTES]
   DeBruijnClz | src/legacy/zstd_v02.c:297-297 | 32 | dead branch: #else (line 296) of #if defined(_MSC_VER) (line 291)
   ```

   | Reason | Cause |
   |---|---|
   | `whole file under DIRECTIVE (line N)` | No compilation unit of the binary has the file, and these `#if` branches have all code of the file. An include guard is not named. If there are two or more, one of them is false in this build. |
   | `file not in the binary (not compiled, or compiles to nothing)` | No compilation unit of the binary has the file, and no `#if` branch has all its code. |
   | `dead branch: DIRECTIVE (line N)` | The array is in this `#if` branch, and the binary has no code from this branch. It is the outermost such branch. |
   | `function not emitted: NAME` | The binary does not have the function (for example an unused `static inline` function). |
   | `cause not found` | The tool found no cause. |

5. `== REPETITIVE ==`. Regions of repeated code. One line for each region,
   then two sample lines (the first line and a middle line). The region line
   also shows the most common first token of a line, the number of lines
   that start with it, and their share, for example `#define: 180 lines (95%)`
   or `ITEM: 25 lines (74%)`. The number of lines is the count of items of the
   main pattern. Use it, and do not count the items in a sample.

An array is in the report if its source text has 30 or more items. This
is also true for a stack array. The binary values do not count. Thus a
short initializer that zero fill or macros make large is not in the report.
An array that is `not in text` has no text items. It is in the report if it
has 30 or more binary values. A single struct or scalar variable is not in
the report. `translate` accepts all arrays, also arrays that are not in the
report.

### Redundancy

Redundancy is the share of repeated token patterns in a region. The tool
replaces each name with one symbol and each number with one symbol. Then it
counts how many 8-token sequences occur more than one time.

The detector shows more regions than necessary on purpose. You decide which
regions are worth a script.

| Redundancy | Meaning |
|---|---|
| below 0.70 | Normal code. Not reported. |
| 0.70 to 0.85 | Maybe repetitive. Look at the sample lines. |
| 0.85 and more | Strongly repetitive. A script can usually translate it. |

Function declaration lists and blocks of similar `if` statements can also
have a high redundancy. They are usually not worth a script.

## Defaults of translate

With no options, the tool makes all choices. The output compiles on its own.

| Item | Default |
|---|---|
| Rust name | The C name in UPPER_SNAKE_CASE. An array inside a function gets the function name as a prefix: `js_strtol` + `table` gives `JS_STRTOL_TABLE`. If two arrays of one command get the same name, each one gets the name of its C file as a prefix (`hash_sha256_cp.c` + `Krnd` gives `HASH_SHA256_CP_KRND`). |
| Item kind | `pub static` |
| Integer types | By size and sign: `u8`, `i16`, `u32`, `i64`, and so on. |
| `char` | `core::ffi::c_char`. A `char` array becomes `[u8; N]` with a byte string `*b"..."`. |
| `bool`, `float`, `double` | `bool`, `f32`, `f64`. The literal gives the exact same bits. It has the shortest digits. It has the plain form (`3600.0`) if that form is not more than 3 characters longer than the exponent form, else the exponent form (`4.6427e14`, `1.0e-300`). |
| enum | Its integer type. The value is the number. |
| struct | The tool writes a `#[repr(C)]` struct in the output file. It has the C name. The field names are the C field names in snake_case (`nextState` gives `next_state`). A field name that is a Rust keyword gets the prefix `r#`, for example `r#type`. An anonymous struct gets the name `<array>_row`, for example `a_xform_type_row` for `aXformType`. |
| `char *` | `&'static core::ffi::CStr`, written as `c"..."`, for string literals. A pointer into a named array (also a `char` array) is a reference `&OTHER[i]`. If one item is NULL, the type is `Option<...>`. |
| pointer to another array | `&OTHER[i]`. If one item is NULL, the type is `Option<...>`. |
| Number format | Hexadecimal if the C source uses mostly hexadecimal. Else decimal. |
| Check | `rustc` compiles the output when the output has no `crate::` paths. |

## Spec file

Use a spec file only for choices that the defaults do not give. All keys are
optional. Write the C name exactly as the report shows it (typedef names
are accepted). A `structs` key can also be the default Rust name of the
struct, or `KEY[]` (the element struct of the array KEY). Use these two
forms for an anonymous struct, because it has no C name.

```json
{
  "defaults": {"kind": "static", "vis": "pub(crate)", "radix": "auto"},
  "symbols": {
    "ZSTD_ldm_gearTab": {"name": "GEAR_TAB", "radix": "hex", "per_line": 4},
    "png_sRGB_table": {"export_name": "png_sRGB_table"}
  },
  "types": {"char": "u8"},
  "structs": {
    "ZSTD_seqSymbol": {"rust": "crate::decompress::SeqSymbol", "fields": {"baseValue": "base"}}
  },
  "enums": {"ZSTD_strategy": {"rust": "crate::Strategy", "variants": {"ZSTD_fast": "Fast"}}},
  "pointers": {"other_table": "crate::tables::OTHER_TABLE"},
  "strings": "cstr",
  "char_arrays": "bytes"
}
```

| Key | Values | Effect |
|---|---|---|
| `defaults` | object | Options for all arrays (`kind`, `vis`, `radix`, `per_line`). |
| `symbols.KEY.name` | Rust identifier | Rust name of the array. Same as `--name`. |
| `symbols.KEY.kind` | `static`, `const` | Item kind. Use `static` for large tables. |
| `symbols.KEY.vis` | `pub`, `pub(crate)`, `""` | Visibility. |
| `symbols.KEY.radix` | `auto`, `hex`, `dec` | Number format. |
| `symbols.KEY.per_line` | integer | Numbers on one line. 0 means automatic. |
| `symbols.KEY.export_name` | C symbol name | Adds `#[unsafe(export_name = "...")]`. Use it when C code or a test links to the table by name. |
| `types.CTYPE` | Rust scalar type | Replaces the default type of a C scalar type. The value keeps its bits. |
| `structs.CTYPE.rust` | Rust path | Use your own struct. The tool does not write a struct definition. CTYPE is a C type name, the default Rust name, or `KEY[]`. |
| `structs.CTYPE.fields` | object | C field name to Rust field name, for the fields that do not use the default (the C name in snake_case). |
| `structs.CTYPE.ctor` | function name | Writes `ctor(a, b, c)` instead of a struct literal. The function must be a `const fn`. |
| `structs.CTYPE.define` | `true`, `false` | Write the struct definition or not. Default: `false` if `rust` is set, else `true`. |
| `enums.CTYPE.rust` | Rust path | Writes `Path::VARIANT` instead of a number. |
| `enums.CTYPE.variants` | object | C enumerator name to Rust variant name. Default: the C name. |
| `pointers.CSYMBOL` | Rust path | Rust path of an array that is not in this command. |
| `strings` | `cstr`, `bytes`, `bytes_nul`, `str` | Type of `char *` items: `&CStr` (default), `&[u8]` without or with the final NUL, or `&str`. With `str`, a string that is not UTF-8 stops the translation of its table with a message. |
| `char_arrays` | `bytes`, `numbers` | `char` arrays as a byte string, or as numbers. With `bytes`, a `char *` that points into a `char` array is `&u8`. |

### Example: rename a struct and its fields

This spec writes the struct `SeqSymbol` into the output file. Without
`"define": true`, the tool expects `SeqSymbol` in your crate. The fields get
the default names (`next_state`, `nb_additional_bits`, `nb_bits`), except
`baseValue`, which becomes `base`.

```json
{
  "structs": {
    "ZSTD_seqSymbol": {"rust": "SeqSymbol", "define": true, "fields": {"baseValue": "base"}}
  },
  "symbols": {"OF_defaultDTable": {"name": "OF_DEFAULT_DTABLE"}}
}
```

```
python3 <SKILL_DIR>/scripts/tab2rust.py translate --src path/to/test_case --binary work/c-build/libzstd.so \
    --symbol OF_defaultDTable --spec spec.json --out src/seq_tables.rs
```

A field name that is a Rust keyword also gets the prefix `r#` in `fields`:
`{"type": "type"}` gives `r#type`.

### Example: rename an anonymous struct

sqlite `date.c` has this table:

```c
static const struct {
  u8 nName; char zName[7]; float rLimit; float rXform;
} aXformType[] = { { 6, "second", 4.6427e+14, 1.0 }, ... };
```

The struct has no C name. Its default Rust name is `a_xform_type_row`. Use
the key `aXformType[]` (or `a_xform_type_row`):

```json
{
  "structs": {
    "aXformType[]": {"rust": "XformType", "define": true}
  }
}
```

The output has `pub struct XformType` and items such as
`XformType { n_name: 6, z_name: *b"second\x00", r_limit: 4.6427e14, r_xform: 1.0 }`.
If a `structs` key matches no struct, `translate` prints a `NOTE`.

If your struct has different field types (for example an enum instead of an
integer), the `rustc` check of your crate shows the error. Then change your
struct or add a `types` entry.

## Output of translate

The command prints one line for each array and does not print the Rust code:

```
wrote src/tables.rs (26912 bytes)
  OK      ZSTD_ldm_gearTab -> ZSTD_LDM_GEAR_TAB: [u64; 256] (256 values, C const U64 [256])
          = [0xf5b8f72c5f77775c, 0x84935f266b7ac412, 0xb647ada9ca730ccc, ..., 0x2b4da14f2613d8f4]
  OK      pso_list -> PSO_LIST: [pso; 23] (92 values, C const pso [23])
          = [pso { name: c"UTF8)", length: 5, r#type: 0, value: 524288 }, pso { name: c"UTF)", length: 4, r#type: 0, value: 524288 }, pso { name: c"UCP)", length: 4, r#type: 0, value: 131072 }, ..., pso { name: c"BSR_UNICODE)", length: 12, r#type: 4, value: 1 }]
          notes: macro items (STRING_UTFn_RIGHTPAR)
  STRUCT  ZSTD_seqSymbol (defined in the output file)
  REFUSED blockCompressor: TAINTED: type (function pointer)
  NOTE    pointer target "x" is not in this command ...
  CHECK   rustc: the file compiles on its own
```

- `OK`: the array is in the file. The line gives the Rust name and the Rust
  type. The next line gives the first values and the last value, as they
  are in the file. A struct element is complete. In an array of arrays or
  structs, the line shows the first and the last element. Arrays inside an
  element show their first three items and their last item. You do not
  need to open the file. A `notes` line gives
  the notes of the array. The output file has them as `// NOTE:` comments.
- `REFUSED`: the array is not in the file. The line gives the reason.
- `NOTE`: something that you must check.
- The exit code is 1 if one array is refused or the `rustc` check fails.

## When the tool refuses

| Message | What to do |
|---|---|
| `TAINTED: stack (...)` | Translate the array by hand. The reason tells why the static copy failed. If the initializer uses a variable, the Rust code must compute it at run time too. |
| `TAINTED: type (union)` (or `bit-field`, `function pointer`, `void pointer`, `long double`) | Translate the array by hand. |
| `TAINTED: count (text N, binary M: no known cause)` | Read the source of the array. Translate it by hand. Use `dump --flat` to get the values. |
| `matched 0 variables (not in this binary: DEAD, or a wrong key)` | The key is wrong, or the array is not in this binary. Copy the key from the report. |
| `matched N variables. Choices: ...` | Two arrays have the same name. Use one of the printed keys. |
| `pointer ... does not point at a whole element` | Translate the array by hand. |
| `a char pointer field has string literals and pointers into arrays` | One Rust type cannot hold both. Translate the array by hand. |
| `a number holds an address` | The C code casts a pointer to a number. Translate the array by hand. |
| `rustc FAILED` | Usually a `types` or `structs` choice that does not fit. Remove it or fix it. |
