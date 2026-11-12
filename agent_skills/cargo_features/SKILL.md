---
name: cargo_features
description: Prints the Cargo feature names of the build options of a C project and checks that Cargo.toml declares them. Use it when you name, write or change Cargo features or cfg(feature = "...") code in a translation of a C project.
license: none
metadata:
  version: "0.1"
---

# cargo_features

The naming rule is the "Cargo features" invariant of your prompt. This tool applies it.

## The options

When `inputs/c_src/configuration.json` exists, the tool reads the options and their values from
it, and you give no options. Otherwise the build options are the `option(...)` and
`set(... CACHE ...)` variables of the CMake files. Get their values from the CMake code that uses
them, for example `if(NAME STREQUAL "v")` or `add_subdirectory(${NAME})` and the directories that
it can select, not from the description text of the option. Give them to the tool:
`--option NAME=V1,V2,...` for a string or integer option, and `--bool NAME` for a boolean option.

## Commands

Run the commands from the root of your checkout. Below, `TOOL` is
`python3 inputs/agent_skills/cargo_features/scripts/cargo_features.py`. Give the same `--option`
and `--bool` arguments to each command.

- `TOOL names` prints the option features as `[features]` lines. Copy them into `Cargo.toml` and
  add `default`.
- `TOOL check [Cargo.toml]` compares the `[features]` of `Cargo.toml` with the option features,
  and prints each missing feature and what to change. Exit status 1 means a problem. Other
  features of the crate are permitted.
