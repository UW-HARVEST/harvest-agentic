# quality_agentic: the quality stage

Stage `quality`/`q` of the agentic pipeline (experimental; bare `--agentic` never runs it). An
agent refactors a translated crate toward safe, idiomatic Rust (fewer operations that need
`unsafe`, fewer clippy and rustc lints) without changing its behavior. Claude Code and OpenCode
only.

## Files

| Path | Content |
|---|---|
| `src/lib.rs` | The framework: materializes the crate and the C source, backs up the regression tests, writes the skill, the role task files and `QUALITY.md`, invokes the agent, restores changed tests, freezes the result |
| `src/prompt_quality.md` | The main agent's prompt: the manager's flow in seven steps |
| `src/quality_md.md` | The template of `QUALITY.md`, the manager's plan file |
| `src/rules.md` | The rules for every change; the framework puts them into `QUALITY.md` and into each role task |
| `src/runner.md`, `worker.md`, `scout.md`, `implementer.md`, `integrator.md` | The task files of the sub-agent roles (`inputs/prompts/QUALITY_*.md` at run time) |
| `skill/` | The `rust_quality` agent skill, written to `inputs/agent_skills/rust_quality/`: `SKILL.md` (the tools), `RECIPES.md` (local refactorings), `GLOBAL.md` (global refactorings), `CURSORS.md` (the cursor model), `scripts/` |

## Tools

- `qscan.py` measures each function (unsafe operations as DARPA counts them, lints, pseudo-safe
  functions) and suggests recipes from a fixed catalogue; it also plans the local units, reports
  the root causes for global work, and compares two scans. It parses Rust with tree-sitter in a
  venv that it creates on first use.
- `regress.py` is the regression gate: the tests that the verify stage left in the crate
  (`verify_env/` unit tests, a replay of saved fuzz inputs, short fuzz smoke campaigns,
  `cargo test`), compared with a recorded baseline.
- `qflow.py` runs the fixed steps: the baseline with autofix, and the integration of a local
  wave.

## Flow

The main agent only decides and audits; sub-agents build, test, integrate and fix, and only
results that pass the gate are merged.

1. A runner records the baseline and applies the safe autofixes (`qflow.py baseline`).
2. Local phase: workers refactor disjoint units in batches of functions; a runner merges each
   wave through the gate (`qflow.py integrate`).
3. Global phase: scouts study the root causes (`qscan.py roots`) and write data-model proposals
   with an additive contract; the manager chooses; implementers carry out the sub-tasks on their
   own branches; integrators merge them through the gate.
4. A final integrator merges the accepted proposals and does the crate-level cleanup; the manager
   writes `QUALITY_REPORT.md`.

Without a verify stage before it the crate has no regression tests, and the gate checks only the
build and the exported symbols. The framework restores `verify_env/` and `tests/` if the agent
changed them.
