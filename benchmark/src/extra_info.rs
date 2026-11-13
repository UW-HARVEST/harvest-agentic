//! `--extra-info SOURCE=PATH`: results of an analysis of the C programs that the user already
//! has. The translate stage gives them to its agents.

use std::path::{Path, PathBuf};
use std::str::FromStr;

/// The `--extra-info` value: a source of information, with its PATH.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtraInfo {
    /// Reports of a list analysis (the singly linked list types of a program): one report file,
    /// or a directory with one `<program directory name>.json` for each program.
    Sll(PathBuf),
}

impl FromStr for ExtraInfo {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, String> {
        let (source, path) = value
            .split_once('=')
            .ok_or("write SOURCE=PATH, for example sll=./reports")?;
        let info = match source {
            "sll" => ExtraInfo::Sll(PathBuf::from(path)),
            _ => return Err(format!("unknown source {source:?}: the only source is sll")),
        };
        if path.is_empty() {
            return Err(format!("PATH is empty: write {source}=<file or directory>"));
        }
        if !Path::new(path).exists() {
            return Err(format!("{path} does not exist"));
        }
        Ok(info)
    }
}

/// The list report of one program, as an absolute path, from the PATH of `--extra-info sll=PATH`.
/// A file is the report of the only program of the run. A directory holds `<program>.json` for
/// each program that has a report; a program without one runs as without the option.
pub fn sll_report(path: &Path, program: &str, programs: usize) -> Result<Option<PathBuf>, String> {
    let file = if path.is_dir() {
        let file = path.join(format!("{program}.json"));
        if !file.is_file() {
            log::info!("No sll info for {program}");
            return Ok(None);
        }
        file
    } else if programs > 1 {
        return Err(format!(
            "--extra-info sll={} is one report file, but the run has {programs} programs. Give a \
             directory with one <program>.json for each program.",
            path.display()
        ));
    } else {
        path.to_path_buf()
    };
    match std::path::absolute(&file) {
        Ok(file) => Ok(Some(file)),
        Err(e) => Err(format!("cannot make {} absolute: {e}", file.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_source_and_an_existing_path() {
        assert_eq!("sll=.".parse(), Ok(ExtraInfo::Sll(PathBuf::from("."))));
        for (value, problem) in [
            ("sll", "SOURCE=PATH"),
            ("./reports", "SOURCE=PATH"),
            ("other=.", "unknown source \"other\""),
            ("sll=./no/such/path", "./no/such/path does not exist"),
            ("sll=", "PATH is empty"),
        ] {
            let err = value.parse::<ExtraInfo>().unwrap_err();
            assert!(err.contains(problem), "{value}: {err}");
        }
    }

    #[test]
    fn directory_gives_each_program_its_own_report() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("lists.json"), "{}").unwrap();
        let report = sll_report(dir.path(), "lists", 2).unwrap().unwrap();
        assert!(report.is_absolute());
        assert!(report.ends_with("lists.json"), "{}", report.display());
        // A program without a report runs as without the option.
        assert_eq!(sll_report(dir.path(), "tables", 2), Ok(None));
    }

    #[test]
    fn file_is_the_report_of_a_single_program() {
        let file = tempfile::NamedTempFile::new().unwrap();
        // The name of the file does not have to be the name of the program.
        let report = sll_report(file.path(), "lists", 1).unwrap();
        assert_eq!(report.as_deref(), Some(file.path()));
        let err = sll_report(file.path(), "lists", 2).unwrap_err();
        assert!(err.contains("2 programs"), "{err}");
    }
}
