use std::fmt;
use std::str::FromStr;
use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Which external agent to invoke for agentic translation and verification.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    #[default]
    Kiro,
    Claude,
    OpenCode,
}

/// One agentic pipeline stage. Stages compose in pipeline order
/// (translate < verify < conform); a run executes an ordered subset.
/// `Conform` is driven by the benchmark binary and never enters
/// [`crate::HarvestIR`]-based scheduling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    Translate,
    Verify,
    Conform,
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Stage::Translate => write!(f, "translate"),
            Stage::Verify => write!(f, "verify"),
            Stage::Conform => write!(f, "conform"),
        }
    }
}

impl FromStr for Stage {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "translate" | "t" => Ok(Stage::Translate),
            "verify" | "v" => Ok(Stage::Verify),
            "conform" | "c" => Ok(Stage::Conform),
            other => Err(format!(
                "unknown stage: {other} (expected: translate/t, verify/v, conform/c)"
            )),
        }
    }
}

impl fmt::Display for AgentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentKind::Kiro => write!(f, "kiro"),
            AgentKind::Claude => write!(f, "claude"),
            AgentKind::OpenCode => write!(f, "opencode"),
        }
    }
}

/// Configuration for this harvest-translate run. The sources of these configuration values (from
/// highest-precedence to lowest-precedence) are:
///
/// 1. Configurations passed using the `--config` command line flag.
/// 2. A user-specific configuration directory (e.g. `$HOME/.config/harvest/config.toml').
/// 3. Defaults specified in the code (using `#[serde(default)]`).
#[derive(Debug, Deserialize)]
pub struct Config {
    /// Path to the directory containing the C code to translate.
    pub input: PathBuf,

    /// Path to output directory.
    pub output: PathBuf,

    /// Path to the diagnostics directory, if you want diagnostics output. If you do not specify a
    /// diagnostics path, a temporary directory will be created (so that working directories can be
    /// created for tools) and cleaned up when translate completes.
    pub diagnostics_dir: Option<PathBuf>,

    /// For both the output directory and diagnostics directory (if enabled):
    /// If true: if the directory exists and is nonempty, translate will delete the contents of the
    /// directory before running.
    /// If false: if the directory exists and is nonempty, translate will output an error and exit.
    pub force: bool,

    /// If true, use modular translation (translating one declaration at a time).
    // If false, use standard all-at-once translation.
    pub modular: bool,

    /// Which agentic stages to run, in pipeline order. Empty means the run is
    /// not agentic (direct LLM translation tools are used instead).
    /// `Stage::Conform` is not valid here (the benchmark drives conform
    /// outside the IR pipeline).
    #[serde(default)]
    pub stages: Vec<Stage>,

    /// When the first stage in `stages` is not `translate`, the path to the
    /// stage-input snapshot: an already-translated program directory
    /// (a previous run's output, carrying a `stage.json` manifest).
    #[serde(default)]
    pub stage_input: Option<PathBuf>,

    /// If true, provide the agent with pre-built analysis tools (c_sandbox, symbol_diff).
    #[serde(default)]
    pub agent_tools: bool,

    /// Which external agent to use for agentic translation (requires `agentic = true`).
    #[serde(default)]
    pub agentic_agent: AgentKind,

    /// Filter describing which log messages should be output to stdout. This is in the
    /// `tracing_subscriber::filter::EnvFilter` format.
    pub log_filter: String,

    /// Sub-configuration for each tool.
    pub tools: HashMap<String, serde_json::Value>,

    // serde will place any unrecognized fields here. This will be passed to unknown_field_warning
    // after parsing to emit warnings on unrecognized config entries (we don't error on unknown
    // fields because that can be annoying to work with if you are switching back and forth between
    // commits that have different config options).
    #[serde(flatten)]
    pub unknown: HashMap<String, serde_json::Value>,
}

impl Config {
    /// Returns a mock config for testing.
    pub fn mock() -> Self {
        Self {
            input: PathBuf::from("mock_input"),
            output: PathBuf::from("mock_output"),
            diagnostics_dir: None,
            force: false,
            modular: false,
            stages: Vec::new(),
            stage_input: None,
            agent_tools: false,
            agentic_agent: AgentKind::default(),
            log_filter: "off".to_owned(),
            tools: Default::default(),
            unknown: Default::default(),
        }
    }

    /// Returns formatted llm info.
    /// Printed at the start of translation and benchmarking runs to aid in reproduction of results.
    pub fn model_info(&self) -> Option<String> {
        if !self.stages.is_empty() {
            let stages = self
                .stages
                .iter()
                .map(Stage::to_string)
                .collect::<Vec<_>>()
                .join("+");
            return Some(format!("agentic({}) stages={stages}", self.agentic_agent));
        }
        let tool_name = if self.modular {
            "modular_translation_llm"
        } else {
            "raw_source_to_cargo_llm"
        };

        self.tools.get(tool_name).map(|tool| {
            let backend = tool
                .get("backend")
                .and_then(Value::as_str)
                .unwrap_or("<unknown>");
            let model = tool
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or("<unknown>");
            let max_tokens = tool
                .get("max_tokens")
                .map_or("<unknown>".to_owned(), |v| v.to_string());

            format!(
                "Backend={} Model={} Max Tokens={}",
                backend, model, max_tokens
            )
        })
    }
}

/// Prints out a warning message for every field in `unknown`.
///
/// This is intended for use by config validation routines. `prefix` should be the path to this
/// entry (e.g. `tools::Config` should call this with a `prefix` of `tools`).
pub fn unknown_field_warning(prefix: &str, unknown: &HashMap<String, Value>) {
    let mut entries: Vec<_> = unknown.keys().collect();
    entries.sort_unstable();
    entries.into_iter().for_each(|name| match prefix {
        "" => eprintln!("Warning: unknown config key {name}"),
        p => eprintln!("Warning: unknown config key {p}.{name}"),
    });
}
