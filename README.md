# HARVEST code

A place to put HARVEST code that has not yet been migrated into its own
repository.

The main components are `harvest_translate` and `harvest_benchmark`, a C-to-Rust translation pipeline, together with a benchmarking wrapper around it.
Besides the one-shot and modular LLM translation paths, it contains an
agentic translator that drives a coding agent through a two-stage translate-then-verify workflow. The agentic
translator is an improvement built on the ideas of
[UW-HARVEST/ACTOR](https://github.com/UW-HARVEST/ACTOR)  re-implemented as native Harvest pipeline
tools with more agent backends and models. Note that the usage differs from
ACTOR. See
[Agentic translation](#agentic-translation) and
[Trace capture and visualization](#trace-capture-and-visualization).

## Building the Rust code

Install `libclang` if it is not already installed.

On Ubuntu, run:

```sh
apt-get install libclang-dev
```

`libclang` is typically bundled with Xcode or Apple's Command Line Tools.
Check for a `libclang` installation by running:

```sh
find /Library/Developer/CommandLineTools /Applications/Xcode.app -name "libclang.dylib" 2>/dev/null
```

Or install it by running either:

```sh
brew install llvm
```

via brew,
    or by running:

```sh
xcode-select install
```

If you have [rustup](https://rustup.rs) installed, you can build the code by
running the following command from the root of this repository:

```bash
cargo build --release
```

If you do not use rustup, you will need a sufficiently-new stable Rust compiler.
See [rust-toolchain.toml](./rust-toolchain.toml) for a version that is known to work.

## LLM server

You will need a local or remote LLM server.
A couple options are given below:

### Local Ollama instance

You can follow Ollama's [download instructions](https://ollama.com/download), or
download its [Docker image](https://hub.docker.com/r/ollama/ollama).

Once you have it installed, you need to download a model.
`harvest_translate` defaults to `codellama:7b`:

```bash
ollama pull codellama:7b                       # If installed in your system
docker container run ollama pull codellama:7b  # If using Docker
```

Ollama must be running to run `harvest_translate`.

### Remote OpenAI instance

First, you'll need to provision an [OpenAI API key](https://platform.openai.com/api-keys).

Then, you'll need to set up a custom Harvest config file:
```toml 
[tools.raw_source_to_cargo_llm]
backend = "openai"
model = "gpt-4o"
api_key = "your_key_here" # Will be read from environment if empty
address = ""  # Not needed for OpenAI
max_tokens = 16384
```
You should place this config at the OS-dependent Harvest config location, which you can find by running:

```sh
cargo run -- --print-config-path
```

## Running

Some of the examples below assume you have a local copy of the TRACTOR
Test-Corpus repository in `export TEST_CORPUS_PATH=/path/to/test-corpus`.

### Translate C code to Rust
```bash
cargo run --bin=translate --release -- /path/to/c/code -o /path/to/output
```

#### Test-Corpus Example:

```bash
cargo run --bin=translate --release -- $TEST_CORPUS_PATH/Public-Tests/B01_synthetic/001_helloworld/test_case/ -o example_output/
```

### Running a set of TRACTOR benchmarks

```bash
cargo run --bin=benchmark --release -- /path/to/input/dir /path/to/output/dir
```

#### Example: run all benchmarks

```bash
cargo run --bin=benchmark --release -- $TEST_CORPUS_PATH/Public-Tests/B01_synthetic example_output/
```

_Optional: add --filter=<regex> to keep only matching benchmarks (by directory name)_

#### Example: run only library benchmarks (directories ending with \_lib)

```bash
cargo run --bin=benchmark --release -- $TEST_CORPUS_PATH/Public-Tests/B01_synthetic example_output/ --filter=".*_lib$"
```

#### Example: run only benchmarks starting with B01

```bash
cargo run --bin=benchmark --release -- $TEST_CORPUS_PATH/Public-Tests example_output/ --filter="^B01"
```

_Optional: add --exclude=<regex> to exclude matching benchmarks (by directory name)_
_Note: --filter and --exclude are mutually exclusive_

### Example: exclude library benchmarks (directories ending with \_lib)

```bash
cargo run --bin=benchmark --release -- $TEST_CORPUS_PATH/Public-Tests/B01_synthetic example_output/ --exclude=".*_lib$"
```

#### Example: exclude benchmarks starting with test\_

```
cargo run --bin=benchmark --release -- $TEST_CORPUS_PATH/Public-Tests example_output/ --exclude="^test_"
```

### Configuration

Print config file location:
```bash
cargo run --bin=translate -- --print-config-path
```

You can find more information on configuration in [doc/Configuration.md].

## Agentic translation

### Two-stage architecture

When `--agentic` is set, the pipeline replaces the one-shot/modular LLM
translation with two agent-driven stages:

1. **Translate** (`tools/translate_agentic`). A coding agent is launched in a
   fresh working directory containing the C source under `c_src/`. It
   translates the project into a Cargo package and iterates until
   `cargo build --release` passes for every feature combination.
2. **Verify & fix** (`tools/verify_fix_agentic`, enabled by
   `--agentic-verify`). A second agent receives the Rust translation together
   with the C source. It compiles the C code as a shared library and uses it
   as the oracle: it writes differential tests, compares
   C and Rust outputs byte-for-byte, and fixes the Rust code until the two agree.

After the agent stages, Harvest freezes the result into its IR and compiles it
one final time (`try_cargo_build`). The `benchmark` binary
validates the result against the corpus test vectors and writes
`results.csv`.

### Supported agents and models

The corresponding agent CLI must be installed and authenticated on your PATH.

**Claude Code** (`--agentic-agent claude`):

- Anthropic models: pass short aliases (`sonnet`, `opus`, `haiku`) or full
  model IDs via `--agentic-model`. Requires a logged-in `claude` CLI.
- Non-Anthropic models via CCR: pass `provider,model` (with a comma), e.g.
  `openrouter,deepseek/deepseek-v4-pro` or `opencode-go,mimo-v2.5`. Harvest
  then routes the `claude` CLI through
  [claude-code-router](https://github.com/musistudio/claude-code-router)
  by setting `ANTHROPIC_BASE_URL=http://127.0.0.1:3456`. Start CCR yourself
  beforehand (with the provider API keys exported in the same shell). Appending `[1m]` to the model name requests the 1M-context
  variant where the provider offers one.

**OpenCode** (`--agentic-agent opencode`, alias `oc`):

- Models use `provider/model` format, e.g. `openrouter/deepseek/deepseek-v4-pro`,
  `opencode-go/mimo-v2.5`, `xiaomi-token-plan-cn/mimo-v2.5-pro`. Any provider
  configured in your OpenCode installation works.
- The model's context/output limits are read from `opencode models --verbose`
  and injected into the prompt so the agent knows its real window.
- After each run, all OpenCode sessions (including sub-agents) are exported
  and appended to the logs, so traces are complete.

**Kiro** (`--agentic-agent kiro`): legacy backend using per-project-kind prompts; kept for comparison.

### Prompt modes

- **Plan mode (default)**: the agent maintains a persistent `PLAN.md`
  (translation) and `HYPOTHESES.md` (verification) on disk as an
  anti-compaction mechanism. After any context compaction it re-reads them to
  recover state. Recommended for large repositories.
- **`--no-plan`**: the original minimal prompts. No plan files, no sub-agent
  guidance. For controlled experiments measuring the impact of the
  anti-compaction mechanism.
- **`--no-plan-file`**: ablation between the two. Keeps the sub-agent
  delegation and context-management guidance, but the prompt never mentions
  plan files or persisting anything to disk. Mutually exclusive with
  `--no-plan`.
- **`--workflow`** (requires `--no-plan`, Claude only): injects a hint asking
  Claude Code to orchestrate the run with its dynamic multi-agent workflow
  feature.

### Test corpus

The benchmarks come from the TRACTOR
[Test-Corpus](https://github.com/UW-HARVEST/Test-Corpus/tree/adapted) (use
the `adapted` branch, which adds real-world libraries such as lz4, jansson,
and zstd under `Adapted-Tests/`). Cloning it into the repository root is the
recommended setup, since the examples below and the toolchain check assume
that location.

Note on toolchains: the repository's `rust-toolchain.toml` must agree with
the Test-Corpus / cando2 required Rust version (currently 1.94.1). This is
checked at startup and the run fails fast on a mismatch, because a mismatched
runner build would otherwise report all test vectors as failures.

### Benchmark CLI flags (agentic)

| Flag | Meaning |
|------|---------|
| `--agentic` | Use the agentic translator instead of one-shot/modular LLM translation |
| `--agentic-verify` | Run the verify-and-fix agent stage after translation |
| `--agentic-agent <A>` | `claude`, `opencode`/`oc`, or `kiro` |
| `--agentic-model <M>` | Model for the agent CLI (see formats above); omit to use the CLI's default |
| `--no-plan` | Minimal prompts without plan files or sub-agent guidance |
| `--no-plan-file` | Sub-agent guidance kept, plan files never mentioned |
| `--workflow` | Hint Claude Code to use dynamic workflows (requires `--no-plan`) |
| `--agent-tools` | Provide the agent with pre-built tools |
| `--wait-until <TS>` | Delay the verify stage until a Unix timestamp (e.g. the next subscription quota window) |
| `--test <PATH>` | Re-validate an already-translated output directory without re-translating |
| `-c, --config K=V` | Override any config value, e.g. `tools.translate_agentic.timeout_secs=7200` |

`--timeout`, `--filter`, and `--exclude` work as in non-agentic benchmark runs.

### Examples

Small public test case, Claude Sonnet, full translate + verify (the trailing
`&>` captures the trace -- see the visualization section below):

```bash
cargo run --bin=benchmark --release -- \
  --agentic --agentic-verify --agentic-agent claude --agentic-model sonnet \
  ./Test-Corpus/Public-Tests/P00_perlin_noise/ ./out_perlin_1_cs &> ./trace_perlin_1_cs.txt
```

A real-world library from the adapted corpus (lz4):

```bash
cargo run --bin=benchmark --release -- \
  --agentic --agentic-verify --agentic-agent claude --agentic-model sonnet \
  ./Test-Corpus/Adapted-Tests/P01_lz4/001_lz4_lib/ ./out_lz4_1_cs &> ./trace_lz4_1_cs.txt
```

Non-Claude model through OpenCode:

```bash
cargo run --bin=benchmark --release -- \
  --agentic --agentic-verify --agentic-agent oc \
  --agentic-model openrouter/deepseek/deepseek-v4-pro \
  ./Test-Corpus/Adapted-Tests/P01_lz4/001_lz4_lib/ ./out_lz4_2_ods4p &> ./trace_lz4_2_ods4p.txt
```

Non-Anthropic model driven by Claude Code through CCR (note the comma):

```bash
cargo run --bin=benchmark --release -- \
  --agentic --agentic-verify --agentic-agent claude \
  --agentic-model "openrouter,deepseek/deepseek-v4-flash" \
  ./Test-Corpus/Public-Tests/P00_perlin_noise/ ./out_perlin_2_cds4f &> ./trace_perlin_2_cds4f.txt
```

Prompt-mode ablations on a large repo (zstd):

```bash
# no-plan
cargo run --bin=benchmark --release -- \
  --agentic --agentic-verify --agentic-agent claude --agentic-model opus --no-plan \
  ./Test-Corpus/Adapted-Tests/P03_zstd/ ./out_zstd_1_co_np &> ./trace_zstd_1_co_np.txt

# workflow mode
cargo run --bin=benchmark --release -- \
  --agentic --agentic-verify --agentic-agent claude --agentic-model sonnet \
  --no-plan --workflow \
  ./Test-Corpus/Adapted-Tests/P01_lz4/001_lz4_lib/ ./out_lz4_3_cs_np_wf &> ./trace_lz4_3_cs_np_wf.txt

# no-plan-file
cargo run --bin=benchmark --release -- \
  --agentic --agentic-verify --agentic-agent claude --agentic-model sonnet --no-plan-file \
  ./Test-Corpus/Adapted-Tests/P01_lz4/001_lz4_lib/ ./out_lz4_4_cs_npf &> ./trace_lz4_4_cs_npf.txt
```

Re-run only the test-vector validation on an existing output:

```bash
cargo run --bin=benchmark --release -- --test ./out_lz4_1_cs --timeout 60
```

Single project through the `translate` binary (no test-vector validation;
model set via config override):

```bash
cargo run --bin=translate --release -- --agentic --agentic-verify \
  --agentic-agent claude --config tools.translate_agentic.model=sonnet \
  ./Test-Corpus/Adapted-Tests/P01_lz4/001_lz4_lib/test_case/ -o out_lz4_translate
```

The `out_*` / `trace_*` naming used above is just a convention:
`<project>_<run#>_<model shorthand>[_np|_npf][_wf]`.

### Output directory contents (agentic runs)

In addition to the standard benchmark outputs (`output.log`, `results.csv`,
per-program translated Rust, `c_src/`, `failed_tests/`, `results.err`),
agentic runs produce per program:

- `plan_translate.md`: the translator's `PLAN.md`, if it wrote one
- `hypotheses_verify.md`: the verifier's `HYPOTHESES.md`, if it wrote one
- `tool_wishlist.json`: static-analysis tools the agent wished it had

## Trace capture and visualization

`parse_trace.py` parses agent traces (both Claude Code (`stream-json`) and
OpenCode (JSONL + session exports) formats, auto-detected) and can render an
SVG timeline of the whole run: one lane per session, turns with their tool
calls, sub-agent brackets, context-compaction markers, and per-session /
grand-total token usage and cost.

### Capturing a trace

The recommended way is to redirect the benchmark's output at launch, as in
all the examples above:

```bash
cargo run --bin=benchmark --release -- --agentic ... ./out_foo &> ./trace_foo.txt
```

The trace file grows while the run is in progress, and the visualizer can be
re-run on the partial file at any time so you can watch the translation live (see the loop below). Alternatively, if you did not redirect,
`<output_dir>/output.log` contains the same agent trace appended after each
stage completes, and can be given to `parse_trace.py` once the run is done.

### Using parse_trace.py

```
python3 parse_trace.py <file> [-v] [-r] [-f] [--format auto|claude|opencode]

  <file>          Path to the trace file (trace_*.txt or output.log)
  (no flags)      Print session statistics (turns, tools, sub-agents, tokens, cost)
  -v, --visualize Generate <file>_timeline.svg — the SVG timeline
  -r, --readable  Generate a human-readable text transcript
  -f, --file-io   Generate a per-file I/O analysis JSON
  --format        Force the trace format (default: auto-detect)
```

Flags can be combined (`-vr`). For OpenCode traces the parser prefers the
embedded session-export blocks over the raw JSONL stream, and falls back when an export is truncated.

### Live visualization loop

To keep visualizations up to date during long runs:

```bash
./auto_visualize_all.bash   # re-runs parse_trace.py -v on all stale trace_* files every 10s
./serve.bash                # serves the directory at http://localhost:8000
```

Open the generated `trace_*_timeline.svg` in a browser tab and refresh to
follow the run's progress in real time.

## Known issue: Claude Code async sub-agents can be killed mid-run

**This problem is mitigated by prompting the agent to only use synchronous sub-agents.**

As of Claude Code 2.1.201 (and some earlier 2.1.x versions), Claude Code
changed the default sub-agent execution mode from synchronous to
asynchronous. In the terminal UI, launching a sub-agent frees the input
box while the sub-agent runs in the background, and its completion is later
delivered to the main agent as a system message.

We suspect this caused a bug for the non-interactive `claude -p` mode
that Harvest uses. When only background sub-agents are running and the main
agent has no foreground task, the state that "frees the input box" in the
terminal UI appears to be treated as the end of the `claude -p` process. The
process then exits and kills still-running sub-agents.

We have observed this in real runs. The main agent ends a turn with
"waiting for the background agents", the process exits, and the trace shows
`task_notification` events with `"status":"stopped"` (instead of
`"completed"`) for sub-agents that were killed mid-translation, leaving
declared Rust modules whose files were never written. If a run fails with
seemingly unfinished work, check the trace for `"status":"stopped"`
notifications to identify this kind of failure.
