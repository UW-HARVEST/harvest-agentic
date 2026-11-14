---
name: spec_contracts
description: Shows the CBMC function contracts of the C library. With the C source and the C headers, you turn them into classes of argument values that tests must cover, that C rejects, that no test passes, or that you cannot decide (unsure). Use it when inputs/extra_info/spec/ exists.
license: none
metadata:
  version: "0.1"
---

# spec_contracts

## What you get

`inputs/extra_info/spec/` holds C files of the library with CBMC function contracts. The C code
in them can differ from `inputs/c_src`. `inputs/c_src` is the reference. A contract is a list of
clauses between the parameter list of a function and its body:

- `__CPROVER_requires(c)`: a precondition. The proof assumes that `c` is true when the function
  starts.
- `__CPROVER_ensures(c)`: a postcondition. The proof shows that `c` is true when the function
  returns.
- `__CPROVER_assigns(...)`: the memory that the function can write.

The proof has limits. It examines only some input sizes, and some clauses only set up the proof.
A clause can be wrong.

Do not read the files in `inputs/extra_info/spec/` directly. Use the commands below. The only
exception: when `show` fails for a function that `list` prints, read the contract of that
function in its file, at the line that `list` prints.

## Commands

Run the commands from the root of your checkout. All paths are relative to it.

```
python3 inputs/agent_skills/spec_contracts/scripts/spec_contracts.py list
python3 inputs/agent_skills/spec_contracts/scripts/spec_contracts.py show <function>
python3 inputs/agent_skills/spec_contracts/scripts/spec_contracts.py notes
python3 inputs/agent_skills/spec_contracts/scripts/spec_contracts.py check DOMAINS.md
python3 inputs/agent_skills/spec_contracts/scripts/spec_contracts.py properties [<function> ...]
```

- `list` prints the functions that have a contract and a declaration in a header of
  `inputs/c_src`. These are the functions with a contract.
- `show <function>` prints the contract of one function:
  - The line of the contract, of each C definition, and of the header declaration.
  - `in the build:` tells whether the build compiles the C definition. See "Conditional code".
  - `header comments that name <function>:` gives the lines of each comment block of the
    headers that has the name of the function.
  - `comments in the contract:` gives the comments between the clauses, with their lines.
  - `macros in the clauses, value in this build:` gives the value of each macro of
    `inputs/c_src` that the clauses use.
  - `clauses:` gives each clause with an id, for example `[3]`. A requires clause `a && b` has
    one id for each part: `[1.1]` and `[1.2]`. A clause `A ==> B` (if A, then B) has a line
    `if:` and a line `then:`. The mark `SETUP` tells that a requires clause only sets up the
    proof. The mark `SETUP in part:` tells that only the part after the mark sets up the proof.
    A `note:` gives a macro that only the contracts use.
  - `clauses by parameter:` gives the ids of the clauses that name each parameter.
  - `constructs in the clauses:` gives the meaning of the `__CPROVER_` words in the clauses.
- `notes` prints the lines and the first line of each header comment block that is not directly
  above a function declaration. Such a comment can hold rules for many functions, and it
  frequently names none of them.
- `check DOMAINS.md` compares `DOMAINS.md` with the contracts, with `inputs/c_src` and with the
  tests in `verify_env/`. It prints one line for each problem, or `No problems found.`
- `properties` prints one line for each FUZZ_TEST in `verify_env/`. The line gives the name, the
  file and the line, the text of `.WithDomains(...)`, and whether the FUZZ_TEST has `.WithSeeds`.
  After `reaches:` it gives the functions with a contract that the property function names,
  directly or through functions of the test files. The command reads names only. It does not
  find a call through a macro or through a table of function pointers. `reaches: none found`
  does not show that the property reaches no such function. With function names, the command
  prints only the properties that reach one of them.

When a command prints a line that starts with `Error:`, correct the command. When you cannot
correct it, report that line as a hypothesis and continue.

## Kinds of classes

A class is a set of argument values of one function. Each class has one kind:

| Kind | Meaning | What you do |
|---|---|---|
| `must cover` | C handles these values and gives a defined result. | Write a test that passes values of the class and compares C and Rust. |
| `C rejects` | C examines these values first and returns an error result. | Write a test that passes values of the class and compares the error results of C and Rust. |
| `never pass` | C has undefined behavior for these values, or the header says that the result is undefined. | Pass them in no test. |
| `unsure` | You cannot decide what C does with these values. | Pass them in no test. Write the class in `DOMAINS.md`. |

## Steps

Do this one time, before the first function:

1. Run `notes`. Read each comment block that it prints, at its lines.

Do this for each function:

1. Run `show <function>`.
2. Read the C definition at the line that `show` prints.
3. Read each header comment that `show` lists, at its lines.
4. For each clause id, decide what the clause says about argument values. Use the rules below.
5. Add the facts that no clause gives, from the header comments and the C code:
   - limits that the C code applies to a value, and default values,
   - values where the C code takes another path,
   - uses of the function that a header comment permits or forbids,
   - special values that the C code accepts.
6. Write the classes of the function as rows of `DOMAINS.md`. See "DOMAINS.md".
7. Write the tests from the classes. See "Rules for the tests".
8. Write the tests of each row into `DOMAINS.md`, and run `check DOMAINS.md`.

## Rules for a requires clause

1. A clause with the mark `SETUP` is not a rule for callers. Take only the buffer sizes from it.
   It does not say that a pointer is not NULL. It does not say that buffers are separate.
2. A clause with the mark `SETUP in part:` has one part that is proof setup. Rule 1 applies to
   that part only. Read the other parts of the clause as a requires clause.
3. A requires clause removes values from the proof. It does not say what C does with these
   values. Find it in the C code: start at the C definition and follow the value to its first
   use.
   - C examines the value first and returns an error result: `C rejects`, with that result.
   - C uses the value first in pointer arithmetic, a dereference, an array index, a shift or a
     division: `never pass`.
   - A header comment says that the result is undefined or corrupted: `never pass`.
   - You cannot find it: `unsure`.
4. A requires clause never removes a value that a header comment or the C code permits. Such a
   value is `must cover`.
5. A requires clause `A ==> B` removes values only for the inputs that make A true.

## Rules for an ensures clause

1. An ensures clause is true only for the inputs that obey the requires clauses. A requires
   clause or a part of it with the mark `SETUP` does not count. This applies to a clause
   `A ==> B` and to a clause without `==>`. For each other input, the clause says nothing, and
   the C code gives the result. Do not report a disagreement for such an input.
2. `A ==> B`: the inputs of rule 1 that make A true are a class, and B is its C result. When B
   is an error result, the kind is `C rejects`. If not, the kind is `must cover`.
3. A clause without `==>` is true for each input of rule 1. Assert it for C and for Rust when
   you can.
4. A macro in a clause has the value that `show` prints for this build. Example: when `show`
   prints `FOO_MODE = 0`, the part `(FOO_MODE && ...)` is false.
5. When a clause and the C code disagree for an input of rule 1, follow the C code. Report the
   disagreement as a hypothesis.

## Other clauses

1. An assigns clause gives the bytes that the function can write. Compare these bytes between
   C and Rust.
2. A clause with `__CPROVER_memory_leak` is a check of the proof. It says nothing about argument
   values. Ignore it.
3. `SETUP` marks are on requires clauses only. In an ensures clause, `__CPROVER_is_fresh` is a
   result of the function: `constructs in the clauses:` gives its meaning.

## Conditional code

Do not decide from the `#if` lines alone which code the build compiles. Use the lines that
`show` prints:

- `in the build: yes`: the build compiles this C definition.
- `in the build: no`: the build does not compile this C definition. When the function has no
  definition in the build, write no class and no test for it.
- `in the build: unknown`: the command could not use the compiler. It prints the `#if`
  conditions around the definition. Compare them with the compile definitions in
  `inputs/c_src/CMakeLists.txt`. When you cannot decide, the kind of each class of the function
  is `unsure`.
- A macro value `unknown`: the command could not use the compiler. Do not assume a value. A
  class that changes with the value is `unsure`.

## Rules for the tests

1. A FUZZ_TEST domain produces each `must cover` class that is a range of values.
2. A test passes values of each `C rejects` class and compares the error results of C and Rust.
3. No TEST and no FUZZ_TEST passes a value of a `never pass` class or of an `unsure` class.
   Such a value is not a generic boundary, also when other instructions ask for values one step
   past a valid range.
4. A precondition is a duty of the caller. Do not add a check, an assert or a panic to the Rust
   code because of a clause.
5. A function without a contract is not in the scope of this skill. Test it as your other
   instructions tell.

## DOMAINS.md

`DOMAINS.md` is in the root of the checkout, with the other surface files. It has one row for
each class. `inputs/agent_skills/spec_contracts/references/domains-table.md` gives the columns
and the rules of a row. Before you finish, run
`python3 inputs/agent_skills/spec_contracts/scripts/spec_contracts.py check DOMAINS.md` and
correct each problem.
