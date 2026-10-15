# Benchmark
The `benchmark` tool is our custom integration-testing and benchmarking utility.
It takes as input a directory of benchmark programs (using the format described below) and produces an output directory containing translated Rust code, debugging information, and summary statistics.  

### Command-line Interface
The command-line interface of `benchmark` is:
```
Usage: benchmark [OPTIONS] <INPUT_DIR> <OUTPUT_DIR>

Arguments:
  <INPUT_DIR>   Path to the directory containing example subdirectories (each with test_case/ and test_vectors/)
  <OUTPUT_DIR>  Path to the output directory for all translated Rust projects

Options:
  -c, --config <CONFIG>    Set a configuration value; format $NAME=$VALUE
      --timeout <TIMEOUT>  Timeout in seconds for running test cases [default: 10]
      --filter <FILTER>    Filter benchmarks by regex pattern on directory names (keeps matching directories)
      --exclude <EXCLUDE>  Exclude benchmarks by regex pattern on directory names (removes matching directories)
  -h, --help               Print help
```

This interface should be fairly intuitive.
The most important detail is that `benchmark` inherits all translation settings (e.g., LLM model choice) from your existing `translate` configuration file.
Only the input and output directories are determined by the command-line arguments.
If you need to override a configuration value, you can use the `--config` flag, which behaves exactly the same way as in translate.

#### Using the `--filter` and `--exclude` options
The `--filter` option takes a regular expression pattern and only runs benchmarks whose directory names match the pattern.
The `--exclude` option takes a regular expression pattern and excludes benchmarks whose directory names match the pattern.

**Note:** The `--filter` and `--exclude` options are mutually exclusive and cannot be used together.

**Filter examples:**

```bash
# Run only library benchmarks (directories ending with _lib)
benchmark Test-Corpus/Public-Tests/B01_synthetic output/ --filter=".*_lib$"

# Run only benchmarks starting with B01
benchmark Test-Corpus/Public-Tests output/ --filter="^B01"

# Run only benchmarks containing "example" in the name
benchmark Test-Corpus/Public-Tests output/ --filter=".*example.*"
```

**Exclude examples:**

```bash
# Exclude library benchmarks (directories ending with _lib)
benchmark Test-Corpus/Public-Tests/B01_synthetic output/ --exclude=".*_lib$"

# Exclude benchmarks starting with test_
benchmark Test-Corpus/Public-Tests output/ --exclude="^test_"

# Exclude benchmarks containing "skip" in the name
benchmark Test-Corpus/Public-Tests output/ --exclude=".*skip.*"
```


### Input Format
The expected directory structure of the `INPUT_DIR` of `benchmark` is as follows:
```
.
├── 001_helloworld
│   ├── test_case
│   │   ├── <build files>
│   │   └── src
│   │       └── main.c
│   └── test_vectors
│       ├── test1.json
│       ├── ...
│       └── test3.json
├── 002_stdin_echo
└── ...
```
This mirrors the format used in the TRACTOR benchmark repository (e.g., `Test-Corpus/Public-Tests/B01_synthetic`). The layout is mostly self-explanatory, but additional documentation can be found in the TRACTOR repository’s README.


### Output Format
The output directory structure of the `OUTPUT_DIR` of `benchmark` is as follows:
```
.
├── output.log
├── results.csv
├── quality.csv
├── 001_helloworld/
│   ├── Cargo.toml
│   └── src
│       └── main.rs
│   ├── c_src
│   │   └── main.c
│   ├── failed_tests
│   │   └── test01.json
│   ├── results.err
├── 002_stdin_echo/
└── ...
```

- `output.log`: The raw output log, which includes the model used, the token budget, and fine-grained results (every test's result, including expected vs actual output).  
- `results.csv`: Summary statistics for the run, like build success rate and test success rate.  
- `quality.csv`: Code-quality summary, one row per program: unsafe operations (safety) and clippy lints / rustc warnings (idiomaticity). Each program's full report is in its `.harvest/quality.json`, with the unsafe operations listed in `.harvest/unsafe_ops.tsv`. See "Code quality metrics" in the root README.  
- `Cargo.toml` and `src`: The translated rust code.   
- `c_src`: The original C source code, exactly copied from the input.  
- `failed_tests`: Any test cases that failed (copied from the input)  
- `results.err`: Error messages for any failing test cases.  
