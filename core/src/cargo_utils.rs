//! Utilities for working with Cargo manifests and projects.
//!
//! The primary interface is [`CargoToml`]: an in-memory handle to a `Cargo.toml` file that
//! supports batching multiple edits before a single write-back.

use std::fs;
use std::path::Path;
use toml_edit::{Array, DocumentMut, Item, Table, Value};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

// CargoToml

/// In-memory handle to a `Cargo.toml` file.
///
/// Call the editing methods in any order, then [`save`](CargoToml::save) once.
///
/// # Example
/// ```no_run
/// # use std::path::Path;
/// # use harvest_core::cargo_utils::CargoToml;
/// let mut cargo = CargoToml::open(Path::new("Cargo.toml")).unwrap();
/// cargo.add_workspace();
/// cargo.set_bin_driver();
/// cargo.save().unwrap();
/// ```
pub struct CargoToml {
    doc: DocumentMut,
    path: std::path::PathBuf,
}

impl CargoToml {
    /// Opens and parses the `Cargo.toml` at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path)?;
        let doc: DocumentMut = content
            .parse()
            .map_err(|e| format!("failed to parse {}: {e}", path.display()))?;
        Ok(Self {
            doc,
            path: path.to_path_buf(),
        })
    }

    /// Writes the in-memory document back to disk.
    pub fn save(&self) -> Result<()> {
        fs::write(&self.path, self.doc.to_string())?;
        Ok(())
    }

    /// Returns the value of `[package].name` from the in-memory document.
    pub fn package_name(&self) -> Option<String> {
        self.doc
            .get("package")?
            .get("name")?
            .as_str()
            .map(|s| s.to_string())
    }

    /// Returns the `[lib].name` if explicitly set, otherwise falls back to `package_name`.
    pub fn lib_name(&self) -> Option<String> {
        self.doc
            .get("lib")
            .and_then(|l| l.get("name"))
            .and_then(|n| n.as_str())
            .map(|s| s.to_string())
            .or_else(|| self.package_name())
    }

    /// Adds an empty `[workspace]` section if not already present, preventing Cargo from
    /// searching parent directories for a workspace root.
    pub fn add_workspace(&mut self) {
        if self.doc.get("workspace").is_none() {
            self.doc.insert("workspace", Item::Table(Table::new()));
        }
    }

    /// Ensures `"cdylib"` appears in `[lib].crate-type`, preserving any other crate types.
    /// Creates the `[lib]` section if it does not exist.
    pub fn ensure_cdylib(&mut self) {
        let needs_lib_table = !matches!(self.doc.get("lib"), Some(Item::Table(_)));
        if needs_lib_table {
            self.doc.insert("lib", Item::Table(Table::new()));
        }

        let lib_table = self
            .doc
            .get_mut("lib")
            .and_then(Item::as_table_mut)
            .expect("`lib` was just created as a table");
        if let Some(ct) = lib_table.get("crate-type") {
            if let Some(arr) = ct.as_array() {
                if arr.iter().any(|v| v.as_str() == Some("cdylib")) {
                    return;
                }
                let mut new_arr = arr.clone();
                new_arr.push("cdylib");
                lib_table.insert("crate-type", Item::Value(Value::Array(new_arr)));
            } else if let Some(s) = ct.as_str() {
                if s == "cdylib" {
                    return;
                }
                let mut arr = Array::new();
                arr.push(s);
                arr.push("cdylib");
                lib_table.insert("crate-type", Item::Value(Value::Array(arr)));
            } else {
                lib_table.insert("crate-type", Item::Value(Value::Array(cdylib_array())));
            }
        } else {
            lib_table.insert("crate-type", Item::Value(Value::Array(cdylib_array())));
        }
    }

    /// Sets `[lib]` with the given `name` and `crate-type = ["cdylib"]`, overwriting any
    /// existing `crate-type`. Use [`ensure_cdylib`](CargoToml::ensure_cdylib) instead when
    /// other crate types must be preserved.
    pub fn set_lib(&mut self, name: &str) {
        let lib = self
            .doc
            .entry("lib")
            .or_insert_with(|| Item::Table(Table::new()));
        if let Some(t) = lib.as_table_mut() {
            t.insert("name", toml_edit::value(name));
            t.insert("crate-type", toml_edit::value(cdylib_array()));
        }
    }

    /// Ensures there is a `[[bin]]` entry with `name = "driver"`.
    /// If a `[[bin]]` section already exists it is renamed; otherwise a new one is added.
    pub fn set_bin_driver(&mut self) {
        if let Some(bins) = self
            .doc
            .get_mut("bin")
            .and_then(|b| b.as_array_of_tables_mut())
            && let Some(bin) = bins.iter_mut().next()
        {
            bin.insert("name", toml_edit::value("driver"));
            return;
        }
        let mut bin = Table::new();
        bin.insert("name", toml_edit::value("driver"));
        bin.insert("path", toml_edit::value("src/main.rs"));
        let mut arr = toml_edit::ArrayOfTables::new();
        arr.push(bin);
        self.doc.insert("bin", Item::ArrayOfTables(arr));
    }

    /// Removes all `[[bin]]` sections.
    pub fn remove_bin(&mut self) {
        self.doc.remove("bin");
    }

    /// Updates the `path` field for a dependency in `[dependencies]`.
    /// Does nothing if the dependency does not exist or has no `path` field.
    pub fn update_dependency_path(&mut self, dep_name: &str, new_path: &str) {
        let Some(deps) = self
            .doc
            .get_mut("dependencies")
            .and_then(|d| d.as_table_mut())
        else {
            return;
        };
        let Some(dep) = deps.get_mut(dep_name) else {
            return;
        };
        if let Some(t) = dep.as_inline_table_mut() {
            if t.contains_key("path") {
                t.insert("path", Value::from(new_path));
            }
        } else if let Some(t) = dep.as_table_mut()
            && t.contains_key("path")
        {
            t.insert("path", Item::Value(Value::from(new_path)));
        }
    }

    /// Makes every relative `path` dependency absolute, resolved against
    /// `base` (the directory the manifest was written for), so a copy of the
    /// manifest placed elsewhere still finds its path dependencies. Covers
    /// `[dependencies]`, `[dev-dependencies]`, `[build-dependencies]` and
    /// their `[target.*]` variants.
    pub fn absolutize_path_dependencies(&mut self, base: &Path) {
        const SECTIONS: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];
        let absolutize = |table: &mut dyn toml_edit::TableLike| {
            for (_, dep) in table.iter_mut() {
                let path = match &dep {
                    Item::Value(Value::InlineTable(t)) => t.get("path").and_then(|p| p.as_str()),
                    Item::Table(t) => t.get("path").and_then(|p| p.as_str()),
                    _ => None,
                };
                let Some(path) = path.filter(|p| Path::new(p).is_relative()) else {
                    continue;
                };
                let absolute = base.join(path).to_string_lossy().into_owned();
                if let Some(t) = dep.as_inline_table_mut() {
                    t.insert("path", Value::from(absolute));
                } else if let Some(t) = dep.as_table_mut() {
                    t.insert("path", Item::Value(Value::from(absolute)));
                }
            }
        };
        for section in SECTIONS {
            if let Some(deps) = self
                .doc
                .get_mut(section)
                .and_then(|d| d.as_table_like_mut())
            {
                absolutize(deps);
            }
        }
        if let Some(targets) = self
            .doc
            .get_mut("target")
            .and_then(|t| t.as_table_like_mut())
        {
            for (_, target) in targets.iter_mut() {
                for section in SECTIONS {
                    if let Some(deps) = target.get_mut(section).and_then(|d| d.as_table_like_mut())
                    {
                        absolutize(deps);
                    }
                }
            }
        }
    }

    /// Sets `[package].name` to match the last component of `project_dir`, sanitized to a
    /// valid Cargo package name. Does nothing if the name is already correct or the directory
    /// name cannot be determined.
    ///
    /// If a `[lib]` section exists without an explicit `name`, this also pins `[lib].name` to
    /// the *original* package name first — otherwise the lib's name (which Cargo derives from
    /// `package.name` when not set) would silently follow the rename and break any
    /// `use <lib_name>::…` references in `src/main.rs` or other binaries.
    pub fn normalize_name(&mut self, project_dir: &Path) {
        let desired_raw = project_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        if desired_raw.is_empty() {
            return;
        }
        let desired = sanitize_package_name(&desired_raw);
        if desired.is_empty() {
            return;
        }

        // Capture the original package name before we change it so we can pin an
        // implicit [lib].name to it.
        let old_pkg_name = self.package_name();

        // Pin [lib].name to the original package name if it would otherwise change implicitly.
        if let Some(old) = old_pkg_name.as_deref()
            && old != desired
            && let Some(lib) = self.doc.get_mut("lib").and_then(|l| l.as_table_mut())
            && !lib.contains_key("name")
        {
            lib.insert("name", Item::Value(Value::from(old)));
        }

        if let Some(pkg) = self.doc.get_mut("package").and_then(|p| p.as_table_mut())
            && pkg.get("name").and_then(|n| n.as_str()) != Some(&desired)
        {
            pkg.insert("name", Item::Value(Value::from(&desired)));
        }
    }

    /// Sets `[features].default` to the given list. Reserved for multi-config support.
    #[allow(dead_code)]
    pub fn set_default_features(&mut self, features: &[String]) {
        let feat = self
            .doc
            .entry("features")
            .or_insert_with(|| Item::Table(Table::new()));
        if let Some(t) = feat.as_table_mut() {
            let mut arr = Array::new();
            for f in features {
                arr.push(f.as_str());
            }
            t.insert("default", toml_edit::value(arr));
        }
    }
}

fn cdylib_array() -> Array {
    let mut arr = Array::new();
    arr.push("cdylib");
    arr
}

// Standalone utilities

/// Sanitizes a string into a valid Cargo package name:
/// - replaces invalid characters with `_`
/// - prefixes with `_` if the name starts with a digit or `-`
pub fn sanitize_package_name(raw: &str) -> String {
    let mut s: String = raw
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' => c,
            _ => '_',
        })
        .collect();
    if s.starts_with(|c: char| c.is_ascii_digit() || c == '-') {
        s.insert(0, '_');
    }
    s
}

/// Removes `src/main.rs` and the `tests/` directory from a translated library project.
///
/// These files are not needed in a library crate and may cause build errors if left in.
pub fn strip_for_lib(crate_dir: &Path) -> Result<()> {
    let main_rs = crate_dir.join("src/main.rs");
    if main_rs.exists() {
        fs::remove_file(&main_rs)?;
    }
    let tests_dir = crate_dir.join("tests");
    if tests_dir.exists() {
        fs::remove_dir_all(&tests_dir)?;
    }
    Ok(())
}

/// Recursively copies a directory tree from `src` to `dst`.
///
/// Silently succeeds if `src` does not exist. Symlinks are skipped.
pub fn copy_directory_recursive(src: &Path, dst: &Path) -> Result<()> {
    if !src.exists() {
        return Ok(());
    }
    fn recurse(src: &Path, dst: &Path) -> Result<()> {
        fs::create_dir_all(dst)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                continue;
            }
            let target = dst.join(entry.file_name());
            if file_type.is_dir() {
                recurse(&entry.path(), &target)?;
            } else {
                fs::copy(entry.path(), &target)?;
            }
        }
        Ok(())
    }
    recurse(src, dst)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_temp(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_package_name_from_in_memory() {
        let f = write_temp("[package]\nname = \"hello\"\n");
        let cargo = CargoToml::open(f.path()).unwrap();
        assert_eq!(cargo.package_name(), Some("hello".to_string()));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_ensure_cdylib_adds_when_missing() {
        let f = write_temp("[package]\nname = \"mylib\"\n\n[lib]\ncrate-type = [\"rlib\"]\n");
        let mut cargo = CargoToml::open(f.path()).unwrap();
        cargo.ensure_cdylib();
        cargo.save().unwrap();
        assert!(fs::read_to_string(f.path()).unwrap().contains("cdylib"));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_ensure_cdylib_no_duplicate() {
        let f = write_temp("[package]\nname = \"mylib\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n");
        let mut cargo = CargoToml::open(f.path()).unwrap();
        cargo.ensure_cdylib();
        cargo.save().unwrap();
        assert_eq!(
            fs::read_to_string(f.path())
                .unwrap()
                .matches("cdylib")
                .count(),
            1
        );
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_ensure_cdylib_creates_lib_section() {
        let f = write_temp("[package]\nname = \"mylib\"\n");
        let mut cargo = CargoToml::open(f.path()).unwrap();
        cargo.ensure_cdylib();
        cargo.save().unwrap();
        let contents = fs::read_to_string(f.path()).unwrap();
        assert!(contents.contains("[lib]"));
        assert!(contents.contains("cdylib"));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_add_workspace() {
        let f = write_temp("[package]\nname = \"test\"\n");
        let mut cargo = CargoToml::open(f.path()).unwrap();
        cargo.add_workspace();
        cargo.save().unwrap();
        assert!(
            fs::read_to_string(f.path())
                .unwrap()
                .contains("[workspace]")
        );
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_add_workspace_no_duplicate() {
        let f = write_temp("[package]\nname = \"test\"\n\n[workspace]\n");
        let mut cargo = CargoToml::open(f.path()).unwrap();
        cargo.add_workspace();
        cargo.save().unwrap();
        assert_eq!(
            fs::read_to_string(f.path())
                .unwrap()
                .matches("[workspace]")
                .count(),
            1
        );
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_update_dependency_path() {
        let f = write_temp(
            "[package]\nname = \"test\"\n\n[dependencies]\ncando2 = { path = \"../old/path\" }\n",
        );
        let mut cargo = CargoToml::open(f.path()).unwrap();
        cargo.update_dependency_path("cando2", "../new/path");
        cargo.save().unwrap();
        let contents = fs::read_to_string(f.path()).unwrap();
        assert!(contents.contains("../new/path"));
        assert!(!contents.contains("../old/path"));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_update_dependency_path_missing_dep() {
        let f = write_temp("[package]\nname = \"test\"\n\n[dependencies]\n");
        let mut cargo = CargoToml::open(f.path()).unwrap();
        cargo.update_dependency_path("nonexistent", "../path"); // must not panic
        cargo.save().unwrap();
    }

    #[test]
    fn test_sanitize_package_name() {
        assert_eq!(sanitize_package_name("hello-world"), "hello-world");
        assert_eq!(sanitize_package_name("123abc"), "_123abc");
        assert_eq!(sanitize_package_name("-foo"), "_-foo");
        assert_eq!(sanitize_package_name("a b.c"), "a_b_c");
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_normalize_name_pins_implicit_lib_name() {
        // Agent named the package `sphincsplus` and source files use
        // `use sphincsplus::...`.  When normalize_name renames the package to
        // match the directory, the implicit lib name must be pinned to the
        // original package name so the source references keep working.
        let dir = tempfile::tempdir().unwrap();
        let project_dir = dir.path().join("_005_renamed_target");
        fs::create_dir_all(&project_dir).unwrap();
        let cargo_path = project_dir.join("Cargo.toml");
        fs::write(
            &cargo_path,
            "[package]\nname = \"sphincsplus\"\n\n[lib]\ncrate-type = [\"lib\", \"cdylib\"]\n",
        )
        .unwrap();

        let mut cargo = CargoToml::open(&cargo_path).unwrap();
        cargo.normalize_name(&project_dir);
        cargo.save().unwrap();

        let contents = fs::read_to_string(&cargo_path).unwrap();
        assert!(contents.contains("name = \"_005_renamed_target\""));
        // The lib name must now be explicitly pinned to the original package name.
        let reopened = CargoToml::open(&cargo_path).unwrap();
        assert_eq!(reopened.lib_name(), Some("sphincsplus".to_string()));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_normalize_name_preserves_explicit_lib_name() {
        // An already-explicit [lib].name must be left alone.
        let dir = tempfile::tempdir().unwrap();
        let project_dir = dir.path().join("renamed");
        fs::create_dir_all(&project_dir).unwrap();
        let cargo_path = project_dir.join("Cargo.toml");
        fs::write(
            &cargo_path,
            "[package]\nname = \"original\"\n\n[lib]\nname = \"my_lib\"\ncrate-type = [\"lib\"]\n",
        )
        .unwrap();

        let mut cargo = CargoToml::open(&cargo_path).unwrap();
        cargo.normalize_name(&project_dir);
        cargo.save().unwrap();

        let reopened = CargoToml::open(&cargo_path).unwrap();
        assert_eq!(reopened.lib_name(), Some("my_lib".to_string()));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_normalize_name_no_lib_section() {
        // No [lib] section at all — nothing should be added.
        let dir = tempfile::tempdir().unwrap();
        let project_dir = dir.path().join("renamed");
        fs::create_dir_all(&project_dir).unwrap();
        let cargo_path = project_dir.join("Cargo.toml");
        fs::write(&cargo_path, "[package]\nname = \"original\"\n").unwrap();

        let mut cargo = CargoToml::open(&cargo_path).unwrap();
        cargo.normalize_name(&project_dir);
        cargo.save().unwrap();

        let contents = fs::read_to_string(&cargo_path).unwrap();
        assert!(!contents.contains("[lib]"));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_absolutize_path_dependencies() {
        let f = write_temp(
            "[package]\nname = \"a\"\n\n\
             [dependencies]\nlocal = { path = \"../local\" }\nreg = \"1\"\nabs = { path = \"/opt/abs\" }\n\n\
             [dependencies.tabled]\npath = \"sub/tabled\"\n\n\
             [target.'cfg(unix)'.dev-dependencies]\nunixy = { path = \"u\" }\n",
        );
        let mut cargo = CargoToml::open(f.path()).unwrap();
        cargo.absolutize_path_dependencies(Path::new("/base/pkg"));
        cargo.save().unwrap();
        let contents = fs::read_to_string(f.path()).unwrap();
        assert!(
            contents.contains("path = \"/base/pkg/../local\""),
            "{contents}"
        );
        assert!(
            contents.contains("path = \"/base/pkg/sub/tabled\""),
            "{contents}"
        );
        assert!(contents.contains("path = \"/base/pkg/u\""), "{contents}");
        assert!(contents.contains("path = \"/opt/abs\""), "{contents}");
        assert!(contents.contains("reg = \"1\""), "{contents}");
    }
}
