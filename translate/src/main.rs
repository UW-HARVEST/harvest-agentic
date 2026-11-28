use clap::Parser;
use harvest_core::utils::empty_writable_dir;
use harvest_translate::cli::{Args, initialize};
use harvest_translate::transpile;
use harvest_translate::util::set_user_only_umask;
use std::sync::Arc;

fn main() {
    if let Err(e) = run() {
        eprintln!("{}", e);
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    set_user_only_umask();
    let args: Arc<Args> = Args::parse().into();
    harvest_translate::cli::check_profile_override(args.baseline, &args.config)?;
    harvest_translate::cli::check_no_dry_run_key(&args.config)?;
    let Some(config) = initialize(args) else {
        return Ok(()); // An early-exit argument was passed.
    };
    empty_writable_dir(&config.output, config.force).expect("output directory error");
    let ir = transpile(config.into())?;
    println!("{}", ir);
    Ok(())
}
