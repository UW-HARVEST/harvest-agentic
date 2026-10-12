# Repetitive structures: write your own mechanical translator

Use this guide for:

- a REPETITIVE region in the detect report,
- a TAINTED array.

The tool cannot translate these. You write a small script that translates
them. The script does the copying, so you do not type the items. This guide
gives the procedure and some templates.

## When to use a script

- Use a script if the region has about 30 or more similar items.
- Translate by hand if the region has fewer items, or if each item is
  different code (for example, a large `switch` with real logic in each
  `case`).
- Ignore a region that is only declarations (function prototypes). Your
  Rust signatures come from the functions themselves.

## Procedure

1. **Look at a sample.** Take the lines of the region from the REPETITIVE
   line of the `detect` report. Do not guess them. Read the first lines, the
   last lines, and some lines in the middle of the region (`sed -n 'A,Bp'`).
   Do not read the full region.
2. **Decide the Rust form before you write code.** Examples: a block of
   `pub const` items, an `enum`, a data table and a loop, or a `match`.
   Decide the Rust type of each value. Use the C type of the declaration.
3. **Write a check script.** It prints only the lines that do not match the
   pattern that you expect, with their line numbers. Typical odd lines are
   `#if`, `#else`, `#endif`, a comment that contains code, a continuation
   line, or one item that is different. Decide what to do with each odd
   line before you continue.
4. **Write a generator script.** It reads the C lines and writes the Rust file.
   Get the values from the compiled C when you can (`tab2rust.py probe` or
   `tab2rust.py dump`), not from the C text. Write the output to the file.
   Do not print the output. For a list of constants, `probe --rust` is the
   generator. You do not need a script.
5. **Check the result with a script.** Compare the names and values in your
   Rust file with the compiled C values. Print only the differences and a
   count. Do not compare by eye.
6. **Translate the odd items by hand**, if there are only a few.

Keep the scripts out of the crate, for example in `/tmp` or in a `tools/`
directory that the build does not use.

## Values from the compiled C

`probe` gives the compiled value of each macro and enum constant:

```
python3 <SKILL_DIR>/scripts/tab2rust.py probe --build <build dir> --tu c_src/src/foo.c \
    --span c_src/src/foo.h:120-300 --rust src/chars.rs
```

- `<SKILL_DIR>` is the directory that contains SKILL.md.
- `--tu` is a `.c` file of the build that includes the header with the
  names. A header is not a correct `--tu`.
- `--span FILE:FIRST-LAST` takes the `#define` names and enum names from
  those lines. `--span FILE` takes them from the whole file. Then add
  `--match REGEX` to select the names, for example `--match '^CHAR_'`.
- `--span` does not find the items of an X-macro list. Give these names as
  arguments.
- You can also give the names as arguments.
- `--rust FILE.rs` writes one `pub const` for each name, with correct Rust
  escapes. The command prints the count of each type, the first constant,
  and the last constant. Do not open the file.
- `--int-type u8` (or another Rust integer type) sets the type of all
  integers. The default is the size and sign of the C type.
- `--str-type bytes` writes strings as `&[u8]`, and `--str-type str` as
  `&str` (UTF-8 strings only). The default is `&CStr`.
- Without `--rust`, the result is JSON: `{"NAME": 42, "S": {"str": "text"}, "F": {"float_bits": "0x3ff0000000000000"}}`.
  Use `--out FILE` to write it to a file.
- The command prints skipped names on stderr. Read the reason:
  - `not defined in this configuration`: the build does not compile the
    `#define` of the name. Do not add the name by hand from another `#if`
    branch. The translation must not have it.
  - `function-like macro`: the name needs arguments. Translate it as a
    function.
  - `not a constant expression`: the name is not a compile-time constant.
    Translate it by hand.

`dump --flat` gives the values of an array, one per line, in memory order.
Use it for a TAINTED array with the reason `count`.

## Template: a list of `#define` constants

C:

```c
#define CHAR_HT   '\t'
#define CHAR_LF   '\n'
#define CHAR_A    'A'
```

Check script. It prints the lines that are not a simple `#define NAME VALUE`:

```python
import re
SRC, FIRST, LAST = 'c_src/src/foo.h', 120, 300
lines = open(SRC, encoding='latin-1').read().split('\n')[FIRST - 1:LAST]
pat = re.compile(r'^\s*#\s*define\s+([A-Za-z_]\w*)\s+\S')
for i, l in enumerate(lines, FIRST):
    if l.strip() and not pat.match(l):
        print(i, l)
```

Generator. `probe --rust` writes the Rust constants. The values come from
the compiled C, so macros that use other macros are correct. If the names
have a common prefix, select them in the whole file with `--match`. Then
you do not need line numbers, and no name is lost at the end of a range:

```
python3 <SKILL_DIR>/scripts/tab2rust.py probe --build <build dir> --tu c_src/src/foo.c \
    --span c_src/src/foo.h --match '^CHAR_' --rust src/chars.rs --int-type u8 --str-type bytes
```

The same name can have a `#define` in two `#if` branches. `probe` gives the
value of the branch that the build compiles.

Check: count the different names with the prefix in the whole file, for
example `grep -oE '#\s*define\s+CHAR_\w+' foo.h | sort -u | wc -l`. The
constants and the skipped names together must give this count. Each name
that is missing must have a skip message on stderr.

## Template: an enum

Use `probe` with `--span` on the enum lines. It gives each enumerator and its
value. Write a `#[repr(i32)]` (or `u32`) Rust enum, or `pub const` items. Use
the C names, or convert them all with one rule. Check that the Rust values
are the `probe` values with a script.

## Template: an X-macro list

C:

```c
#define ERROR_LIST(ITEM) \
    ITEM(ERROR_GENERIC) \
    ITEM(ERROR_maxBlockSize_invalid) \
    ...
```

1. The check script prints the lines that are not `ITEM(NAME) \`.
2. The generator takes the names in order. It writes each expansion that the
   C code uses, for example an enum and a table of name strings.
3. Use `probe` on the expanded names (for example `LZ4F_ERROR_GENERIC`) to
   get or check the enum values. `--span` does not find these names. Give
   them as arguments.

## Template: a table hidden in macro calls

C (16 lines of the same macro with different arguments):

```c
ROUND(m0, cst[1], m1, cst[0], ...);
ROUND(m14, cst[10], m10, cst[14], ...);
```

1. Decide the data. Here it is the index in each argument. Rust form: a
   table `const SIGMA: [[usize; 32]; 16]` and one loop that calls the round
   function.
2. The check script prints the lines that do not match
   `ROUND\((\w+\[?\d*\]?\s*,\s*){N}\w+\[?\d*\]?\);`.
3. The generator extracts the numbers of each line into one row.
4. Check: the number of rows is the number of calls in C. The number of
   iterations of your loop is the number of calls. Print both counts.

## Template: a mechanical switch

C:

```c
case PREFIX(no_error): return "No error detected";
case PREFIX(GENERIC):  return "Error (generic)";
```

The generator writes one `match` arm, or one row of a table, for each `case`
line. The check script prints the lines that are not `case X: return "...";`.
Use `probe` to get the numeric value of each case label if the Rust code
needs it.

## Strings in a generator

If your generator writes Rust strings, use a Rust escape function. Do not use
`json.dumps`. JSON escapes such as `\u0085` are not Rust byte-string escapes.

```python
def rust_bytes(b):  # bytes -> Rust byte-string literal b"..."
    return 'b"' + ''.join(chr(x) if 0x20 <= x < 0x7f and x not in (0x22, 0x5c) else f'\\x{x:02x}'
                          for x in b) + '"'
```

## Do not

- Do not type the items from memory or by eye.
- Do not make a compile error go away by a change of an array length.
  Find the lost or extra item.
- Do not check the result by typing the C values and the Rust values again.
  Use the compiled C values.
