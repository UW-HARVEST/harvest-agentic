//! The command-line arguments and configuration system for [crate::transpile] and HARVEST's
//! `translate` binary.

use clap::Parser;
use config::FileFormat::Toml;
use directories::ProjectDirs;
use harvest_core::config::{Config, Stage};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Command-line arguments for HARVEST's `translate` binary.
#[derive(Debug, Parser)]
pub struct Args {
    /// Set a configuration value; format $NAME=$VALUE.
    #[arg(long, short)]
    pub config: Vec<String>,

    /// Erase the output/diagnostics directories if nonempty.
    #[arg(long, short)]
    pub force: bool,

    /// Use modular translation rather than standard all-at-once translation.
    #[arg(long, conflicts_with = "agentic")]
    pub modular: bool,

    /// Run the agentic pipeline. The optional value selects the stages, in
    /// pipeline order (aliases: t, v): `--agentic=translate`,
    /// `--agentic=verify`, `--agentic=translate,verify`. Bare `--agentic`
    /// means translate,verify. Stages other than translate require
    /// --stage-input.
    #[arg(
        long,
        conflicts_with = "modular",
        num_args(0..=1),
        require_equals = true,
        default_missing_values = ["translate", "verify"],
        value_delimiter = ',',
        value_parser = clap::builder::ValueParser::new(|s: &str| s.parse::<Stage>())
    )]
    pub agentic: Option<Vec<Stage>>,

    /// A previous run's output program directory (carrying stage.json)
    /// to use as the stage input when the first agentic stage is not translate.
    #[arg(long, requires = "agentic")]
    pub stage_input: Option<PathBuf>,

    /// Provide the agent with pre-built analysis tools (requires --agentic).
    #[arg(long, requires = "agentic")]
    pub agent_tools: bool,

    /// Which agent to use for agentic translation: kiro, claude, or opencode (requires --agentic).
    #[arg(long = "agent", requires = "agentic", value_parser = parse_agent_kind)]
    pub agentic_agent: Option<harvest_core::config::AgentKind>,

    /// Path to the directory containing the C code to translate.
    // Should always be present unless using a subcommand like --print-config-path
    pub input: Option<PathBuf>,

    /// Prints out the location of the config file.
    #[arg(long)]
    pub print_config_path: bool,

    /// Path to output directory containing the translated Rust code.
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}

/// Prints out a warning message for every field in `unknown`.
///
/// This is intended for use by config validation routines. `prefix` should be the path to this
/// entry (e.g. `tools::Config` should call this with a `prefix` of `tools`).
pub(crate) fn unknown_field_warning(prefix: &str, unknown: &HashMap<String, Value>) {
    let mut entries: Vec<_> = unknown.keys().collect();
    entries.sort_unstable();
    entries.into_iter().for_each(|name| match prefix {
        "" => eprintln!("Warning: unknown config key {name}"),
        p => eprintln!("Warning: unknown config key {p}.{name}"),
    });
}

fn parse_agent_kind(s: &str) -> Result<harvest_core::config::AgentKind, String> {
    match s.to_lowercase().as_str() {
        "kiro" => Ok(harvest_core::config::AgentKind::Kiro),
        "claude" => Ok(harvest_core::config::AgentKind::Claude),
        "opencode" | "oc" => Ok(harvest_core::config::AgentKind::OpenCode),
        other => Err(format!(
            "unknown agent kind: {other} (expected: kiro, claude, opencode)"
        )),
    }
}

/// Performs parsing and validation of the config; to be called by main() before executing any code
/// that tries to retrieve the config.
///
/// Returns the config, or None if a command line flag that calls for an early exit (such as
/// --print_config_path) was provided.
pub fn initialize(args: Arc<Args>) -> Option<Config> {
    let dirs = ProjectDirs::from("", "", "harvest").expect("no home directory");
    if args.print_config_path {
        println!("Config file location: {:?}", config_file(dirs.config_dir()));
        return None;
    }
    let config = load_config(&args, dirs.config_dir());
    unknown_field_warning("", &config.unknown);
    Some(config)
}

fn load_config(args: &Args, config_dir: &Path) -> Config {
    let mut settings = config::Config::builder()
        .add_source(config::File::from_str(
            include_str!("../default_config.toml"),
            Toml,
        ))
        .add_source(config::File::from(config_file(config_dir)).required(false))
        .add_source(config::File::from(PathBuf::from("config.toml")).required(false));
    for config_arg in &args.config {
        let Some((name, value)) = config_arg.split_once('=') else {
            panic!("failed to parse config value {config_arg:?}; no '=' found");
        };
        // The override pipeline ultimately lands in `tools: HashMap<String,
        // serde_json::Value>`, which uses serde-json's strict deserialization.
        // Passing every value as a String would reject bool/int-typed fields
        // inside nested tool configs. Coerce values that look like typed JSON
        // literals so they reach serde as the right kind. Strings that happen
        // to look like paths/names don't parse as bool/int/float, so they fall
        // through unchanged.
        settings = if let Ok(b) = value.parse::<bool>() {
            settings.set_override(name, b)
        } else if let Ok(i) = value.parse::<i64>() {
            settings.set_override(name, i)
        } else if let Ok(f) = value.parse::<f64>() {
            settings.set_override(name, f)
        } else {
            settings.set_override(name, value)
        }
        .expect("settings override failed");
    }

    if args.force {
        settings = settings
            .set_override("force", "true")
            .expect("settings override failed");
    }

    if args.modular {
        settings = settings
            .set_override("modular", "true")
            .expect("settings override failed");
    }

    if let Some(stages) = &args.agentic {
        // Normalize to pipeline order (Stage's Ord is declaration order).
        let mut stages = stages.clone();
        stages.sort();
        stages.dedup();
        let stages: Vec<String> = stages.iter().map(ToString::to_string).collect();
        settings = settings
            .set_override("stages", stages)
            .expect("settings override failed");
    }

    if args.agent_tools {
        settings = settings
            .set_override("agent_tools", "true")
            .expect("settings override failed");
    }

    if let Some(agent) = args.agentic_agent {
        settings = settings
            .set_override("agentic_agent", agent.to_string())
            .expect("settings override failed");
    }

    // We need to set an override so that deserializing the config does not error.
    // However, the config crate does not support providing a Path in an override.
    // We could convert to a string and back, but that can be lossy. Instead, this just sets a
    // blank value and then corrects it after deserialization.
    if args.input.is_some() {
        settings = settings
            .set_override("input", " ")
            .expect("settings override failed");
    }

    if args.output.is_some() {
        settings = settings
            .set_override("output", " ")
            .expect("settings override failed");
    }

    if args.stage_input.is_some() {
        settings = settings
            .set_override("stage_input", " ")
            .expect("settings override failed");
    }

    let mut config: Config = settings
        .build()
        .expect("failed to build settings")
        .try_deserialize()
        .expect("config deserialization failed");
    if let Some(ref input) = args.input {
        config.input = input.clone();
    }
    if let Some(ref output) = args.output {
        config.output = output.clone();
    }
    if let Some(ref stage_input) = args.stage_input {
        config.stage_input = Some(stage_input.clone());
    }
    config
}

/// Returns the config file path, given the config directory.
fn config_file(config_dir: &Path) -> PathBuf {
    [config_dir, "translate.toml".as_ref()].iter().collect()
}

#[cfg(test)]
mod tests {
    #[cfg(not(miri))]
    #[test]
    fn load_config_test() {
        use super::*;
        use harvest_core::test_util::tempdir;
        use std::{fs, io::Write as _};
        let config_dir = tempdir().unwrap();

        assert_eq!(
            load_config(
                &Args::parse_from(["", "a", "--output=/tmp/out"]),
                config_dir.path(),
            )
            .input,
            AsRef::<Path>::as_ref("a")
        );

        fs::File::create(config_file(config_dir.path()))
            .unwrap()
            .write_all(
                br#"
                    input = "b"
                    [tools.raw_source_to_cargo_llm]
                    address = "127.0.0.1"
                    model = "gpt-oss"
                "#,
            )
            .unwrap();
        assert_eq!(
            load_config(
                &Args::parse_from(["", "--output=/tmp/out"]),
                config_dir.path()
            )
            .input,
            AsRef::<Path>::as_ref("b")
        );
        // Verify the --config flag overrides the user's config file.
        assert_eq!(
            load_config(
                &Args::parse_from(["", "--config", "input=c", "--output=/tmp/out"]),
                config_dir.path()
            )
            .input,
            AsRef::<Path>::as_ref("c")
        );
        // Verify --input overrides all the configuration options.
        assert_eq!(
            load_config(
                &Args::parse_from(["", "--config", "input=d", "d", "--output=/tmp/out"]),
                config_dir.path()
            )
            .input,
            AsRef::<Path>::as_ref("d")
        );
        // Verify --force enables the force option.
        assert!(
            load_config(
                &Args::parse_from(["", "--force", "--output=/tmp/out"]),
                config_dir.path()
            )
            .force
        );
    }
}
