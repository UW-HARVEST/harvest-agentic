# DOMAINS.md

`DOMAINS.md` is the fifth surface file of the verification. It is in the root of the checkout.
It has one table. Each row is one class of argument values of one function with a contract. You
write the rows of the functions that you test.

The table starts with these two lines:

```
| function | parameters | values | kind | C result | source | test | note |
|---|---|---|---|---|---|---|---|
```

When `DOMAINS.md` exists, add your rows to its table. Do not change the rows of other functions.

## Columns

| Column | Content |
|---|---|
| function | The name of one function that the `list` command prints. |
| parameters | The parameters of the class. |
| values | The values of the class: a range, a value, or a relation between parameters. |
| kind | `must cover`, `C rejects`, `never pass` or `unsure`. |
| C result | What C returns or writes for these values. For a `never pass` row, what C does with the value first. |
| source | Where the class comes from: one or more items, with commas between them. No other text. |
| test | The tests that pass values of the class, as `Suite.Name`, with commas between them. |
| note | How the domain or the test produces the class, in a few words. Or `not covered:` and the reason. |

An item of the column `source` is one of these:

- A clause id of the function in brackets, as the `show` command prints it: `[3]` or `[1.2]`.
- A line or a range of lines of the C source: `inputs/c_src/<path>:<line>` or
  `inputs/c_src/<path>:<line>-<line>`.

Write `\|` for a `|` in a cell.

## Rules

1. Name a test in a row only when the test calls the function with values of the class.
2. A `must cover` row or a `C rejects` row names one or more tests. When no test passes values
   of the class, the column `test` is empty, and the note starts with `not covered:` and gives
   the reason.
3. A `never pass` row or an `unsure` row names no test. For an `unsure` row, the note tells what
   you could not decide.
4. Each function with a contract that a test calls has one or more rows.

## Examples

The functions `bar_fill` and `bar_find` and their lines are examples only.

A class that a domain produces:

```
| bar_fill | count | 0 .. BAR_SLOTS | must cover | the number of slots that the call fills | [2.2], inputs/c_src/src/bar.c:88 | BarFill.MatchesC | the domain is InRange(0, BAR_SLOTS) |
```

A class that C rejects:

```
| bar_fill | mode | a value that is not in the enum | C rejects | returns -1 | [3], inputs/c_src/src/bar.c:75 | BarFill.RejectsBadMode | the domain is the values outside 0 .. 2 |
```

A class without a test:

```
| bar_find | key | a key that the table does not hold | must cover | returns NULL | inputs/c_src/include/bar.h:40-52 |  | not covered: no test makes a table without the key |
```

A class that no test passes:

```
| bar_find | table | a table after bar_close | never pass | reads the freed table | [1], inputs/c_src/src/bar.c:131 |  |  |
```

A class that you cannot decide:

```
| bar_fill | count | > BAR_SLOTS, with mode BAR_APPEND | unsure |  | [2.2] |  | the C code has no check, and the header comment does not say |
```

## Before you finish

Run this command:

```
python3 inputs/agent_skills/spec_contracts/scripts/spec_contracts.py check DOMAINS.md
```

It prints `No problems found.` or one line for each problem:

- The first table row is not the header row.
- A row does not have eight cells.
- A kind is not `must cover`, `C rejects`, `never pass` or `unsure`.
- A function has no contract.
- The column `source` is empty.
- A source item is not a clause id of the function and not a line of a file in `inputs/c_src`.
- A name in the column `test` is not a TEST or a FUZZ_TEST in `verify_env/`.
- A `never pass` row or an `unsure` row names a test. Remove the value from that test, and
  remove the name from the row.
- A `must cover` row or a `C rejects` row has no test, and its note does not start with
  `not covered:`.
- The tests name a function with a contract, and the function has no row.

Correct each problem. Do not write `not covered:` for a class that a test passes.
