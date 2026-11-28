# Cargo features

## Build options and features

- `inputs/prompts/output_spec.md` gives the rule: which build options become
  which Cargo features, and with which command a configuration is built. The
  case does not change: `DEBUG` is not `debug`. When
  `inputs/c_src/configuration.json` exists, the Rust code follows the CMake
  default of every other option.
- Do not type feature names. The skill `inputs/agent_skills/cargo_features/`
  prints them (`names`) and checks `Cargo.toml` (`check`). Read its
  `SKILL.md`. Give the printed names to each sub-agent that writes
  `cfg(feature = "...")` code.
- The harness can replace the option features in `default`. Every other
  configuration is built with one feature for each option that the
  configuration sets. Test a configuration with the build command that
  `inputs/prompts/output_spec.md` gives. The translation must compile and be
  correct in every configuration: every value of every option.
- The warning ``unexpected `cfg` condition value`` (lint `unexpected_cfgs`)
  is an error: a `cfg(feature = "...")` names a feature that `Cargo.toml`
  does not declare.
- When the crate has a `Cargo.toml`, run `check` before you end your task.

## Cargo manifest target names

- `[lib] name` and `[[bin]] name` MUST use underscores only — NO hyphens.
  Hyphens in target names cause `cargo` to fail parsing the manifest entirely.
  RIGHT: `name = "sphincs_plus"`, WRONG: `name = "sphincs-plus"`. Fix
  `Cargo.toml` if you see them.
