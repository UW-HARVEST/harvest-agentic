//! `--extra-info SOURCE=PATH`: information about the C programs that the user already has. The
//! stage of the source gives it to its agents: the translate stage for `sll`, the verify stage for
//! `spec`.

use std::path::{Path, PathBuf};
use std::str::FromStr;

/// The `--extra-info` value: a source of information, with its PATH.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtraInfo {
    /// Reports of a list analysis (the singly linked list types of a program): one report file,
    /// or a directory with one `<program directory name>.json` for each program.
    Sll(PathBuf),
    /// C files with function contracts: a directory with one directory
    /// `<program directory name>/` for each program, which holds the annotated C files of the
    /// program.
    Spec(PathBuf),
}

impl FromStr for ExtraInfo {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, String> {
        let (source, path) = value
            .split_once('=')
            .ok_or("write SOURCE=PATH, for example sll=./reports")?;
        let info = match source {
            "sll" => ExtraInfo::Sll(PathBuf::from(path)),
            "spec" => ExtraInfo::Spec(PathBuf::from(path)),
            _ => {
                return Err(format!(
                    "unknown source {source:?}: the sources are sll and spec"
                ));
            }
        };
        if path.is_empty() {
            return Err(format!("PATH is empty: write {source}=<file or directory>"));
        }
        if !Path::new(path).exists() {
            return Err(format!("{path} does not exist"));
        }
        if matches!(info, ExtraInfo::Spec(_)) && !Path::new(path).is_dir() {
            return Err(format!(
                "{path} is not a directory: write spec=<directory>, with one directory \
                 <program>/ for each program"
            ));
        }
        Ok(info)
    }
}

impl ExtraInfo {
    /// The source with the information of one program as its PATH, absolute: the report file for
    /// `sll`, the directory of the annotated C files for `spec`. A program without it runs as
    /// without the option.
    pub fn for_program(&self, program: &str, programs: usize) -> Result<Option<Self>, String> {
        let found = match self {
            ExtraInfo::Sll(path) => sll_report(path, program, programs)?.map(ExtraInfo::Sll),
            ExtraInfo::Spec(path) => spec_dir(path, program)?.map(ExtraInfo::Spec),
        };
        Ok(found)
    }
}

/// The list report of one program. A PATH that is a file is the report of the only program of the
/// run. A PATH that is a directory holds `<program>.json` for each program that has a report.
fn sll_report(path: &Path, program: &str, programs: usize) -> Result<Option<PathBuf>, String> {
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
    absolute(&file).map(Some)
}

/// The directory `<PATH>/<program>/` of one program, when it has one or more annotated C files.
fn spec_dir(path: &Path, program: &str) -> Result<Option<PathBuf>, String> {
    let dir = path.join(program);
    if verify_fix_agentic::spec_c_files(&dir).is_ok_and(|files| !files.is_empty()) {
        absolute(&dir).map(Some)
    } else {
        log::warn!("No spec for {program}");
        Ok(None)
    }
}

fn absolute(path: &Path) -> Result<PathBuf, String> {
    std::path::absolute(path).map_err(|e| format!("cannot make {} absolute: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_source_and_an_existing_path() {
        assert_eq!("sll=.".parse(), Ok(ExtraInfo::Sll(PathBuf::from("."))));
        assert_eq!("spec=.".parse(), Ok(ExtraInfo::Spec(PathBuf::from("."))));
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
    fn spec_path_is_a_directory() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let path = file.path().display();
        let err = format!("spec={path}").parse::<ExtraInfo>().unwrap_err();
        assert!(err.contains("is not a directory"), "{err}");
        // `sll` keeps its file form.
        assert!(format!("sll={path}").parse::<ExtraInfo>().is_ok());
    }

    #[test]
    fn directory_gives_each_program_its_own_report() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("lists.json"), "{}").unwrap();
        let reports = ExtraInfo::Sll(dir.path().to_path_buf());
        let Some(ExtraInfo::Sll(report)) = reports.for_program("lists", 2).unwrap() else {
            panic!("no report for lists");
        };
        assert!(report.is_absolute());
        assert!(report.ends_with("lists.json"), "{}", report.display());
        // A program without a report runs as without the option.
        assert_eq!(reports.for_program("tables", 2), Ok(None));
    }

    #[test]
    fn spec_of_a_program_is_its_own_directory() {
        // Which files of the directory count is tested in verify_fix_agentic (spec_c_files).
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("lists")).unwrap();
        std::fs::write(dir.path().join("lists/lists.c"), "").unwrap();
        let spec = ExtraInfo::Spec(dir.path().to_path_buf());
        let Some(ExtraInfo::Spec(lists)) = spec.for_program("lists", 2).unwrap() else {
            panic!("no spec for lists");
        };
        assert!(lists.is_absolute());
        assert!(lists.ends_with("lists"), "{}", lists.display());
        // A program without the directory runs as without the option.
        assert_eq!(spec.for_program("tables", 2), Ok(None));
    }

    #[test]
    fn file_is_the_report_of_a_single_program() {
        let file = tempfile::NamedTempFile::new().unwrap();
        // The name of the file does not have to be the name of the program.
        let report = ExtraInfo::Sll(file.path().to_path_buf());
        assert_eq!(report.for_program("lists", 1), Ok(Some(report.clone())));
        let err = report.for_program("lists", 2).unwrap_err();
        assert!(err.contains("2 programs"), "{err}");
    }
}
