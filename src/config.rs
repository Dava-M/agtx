use std::collections::HashMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub refresh_debounce_ms: u64,
    pub max_diff_bytes: usize,
    pub max_diff_lines: usize,
    pub show_untracked: bool,
    pub ignore_dirs: Vec<String>,
    pub editor: EditorConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            refresh_debounce_ms: 200,
            max_diff_bytes: 2_000_000,
            max_diff_lines: 20_000,
            show_untracked: true,
            ignore_dirs: vec![
                "target".to_string(),
                "node_modules".to_string(),
                ".git".to_string(),
            ],
            editor: EditorConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorConfig {
    pub default: String,
    pub profiles: HashMap<String, EditorProfile>,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            default: "nvim".to_string(),
            profiles: builtin_profiles(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorProfile {
    pub command: Vec<String>,
}

fn builtin_profiles() -> HashMap<String, EditorProfile> {
    HashMap::from([
        (
            "nvim".to_string(),
            EditorProfile {
                command: vec![
                    "nvim".to_string(),
                    "+{line}".to_string(),
                    "{file}".to_string(),
                ],
            },
        ),
        (
            "helix".to_string(),
            EditorProfile {
                command: vec!["hx".to_string(), "{file}:{line}".to_string()],
            },
        ),
        (
            "vscode".to_string(),
            EditorProfile {
                command: vec![
                    "code".to_string(),
                    "-g".to_string(),
                    "{file}:{line}".to_string(),
                ],
            },
        ),
    ])
}

/// `$XDG_CONFIG_HOME/agtx/config.toml`, else `$HOME/.config/agtx/config.toml`.
pub fn default_path() -> Result<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return Ok(PathBuf::from(xdg).join("agtx").join("config.toml"));
        }
    }
    let home = std::env::var_os("HOME").context("neither XDG_CONFIG_HOME nor HOME is set")?;
    Ok(PathBuf::from(home)
        .join(".config")
        .join("agtx")
        .join("config.toml"))
}

/// An explicit path must exist and parse; `None` falls back to the default
/// location and to built-in defaults when that file is absent.
pub fn load(path: Option<&Path>) -> Result<Config> {
    match path {
        Some(path) => {
            let text = fs::read_to_string(path)
                .with_context(|| format!("cannot read config file `{}`", path.display()))?;
            let config: Config = toml::from_str(&text)
                .with_context(|| format!("cannot parse config file `{}`", path.display()))?;
            validate(&config)
                .with_context(|| format!("invalid config file `{}`", path.display()))?;
            Ok(config)
        }
        None => {
            let path = default_path()?;
            match fs::read_to_string(&path) {
                Ok(text) => {
                    let config: Config = toml::from_str(&text).with_context(|| {
                        format!("cannot parse config file `{}`", path.display())
                    })?;
                    validate(&config)
                        .with_context(|| format!("invalid config file `{}`", path.display()))?;
                    Ok(config)
                }
                Err(err) if err.kind() == ErrorKind::NotFound => Ok(Config::default()),
                Err(err) => Err(err)
                    .with_context(|| format!("cannot read config file `{}`", path.display())),
            }
        }
    }
}

fn validate(config: &Config) -> Result<()> {
    if !config.editor.profiles.contains_key(&config.editor.default) {
        bail!(
            "editor.default profile `{}` is not defined in [editor.profiles]",
            config.editor.default
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_config(dir: &Path, name: &str, text: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn defaults_match_global_constraints() {
        let config = Config::default();
        assert_eq!(config.refresh_debounce_ms, 200);
        assert_eq!(config.max_diff_bytes, 2_000_000);
        assert_eq!(config.max_diff_lines, 20_000);
        assert!(config.show_untracked);
        assert_eq!(config.ignore_dirs, vec!["target", "node_modules", ".git"]);
        assert_eq!(config.editor.default, "nvim");
        assert_eq!(config.editor.profiles.len(), 3);
        assert_eq!(
            config.editor.profiles["nvim"].command,
            vec!["nvim", "+{line}", "{file}"]
        );
        assert_eq!(
            config.editor.profiles["helix"].command,
            vec!["hx", "{file}:{line}"]
        );
        assert_eq!(
            config.editor.profiles["vscode"].command,
            vec!["code", "-g", "{file}:{line}"]
        );
    }

    #[test]
    fn load_without_path_returns_defaults_when_file_absent() {
        let default_file = default_path().unwrap();
        if default_file.is_file() {
            return; // a real config on this machine changes the expectation
        }
        assert_eq!(load(None).unwrap(), Config::default());
    }

    #[test]
    fn toml_override_changes_debounce_ignore_dirs_and_profile() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(
            dir.path(),
            "override.toml",
            r#"
refresh_debounce_ms = 350
ignore_dirs = ["dist", "vendor"]

[editor.profiles.nvim]
command = ["nvim", "+{line}", "{file}", "-u", "NONE"]

[editor.profiles.helix]
command = ["hx", "{file}:{line}"]

[editor.profiles.vscode]
command = ["code", "-g", "{file}:{line}"]
"#,
        );
        let config = load(Some(&path)).unwrap();
        assert_eq!(config.refresh_debounce_ms, 350);
        assert_eq!(config.ignore_dirs, vec!["dist", "vendor"]);
        assert_eq!(
            config.editor.profiles["nvim"].command,
            vec!["nvim", "+{line}", "{file}", "-u", "NONE"]
        );
        assert_eq!(config.editor.default, "nvim");
        assert_eq!(config.max_diff_bytes, 2_000_000);
        assert_eq!(config.max_diff_lines, 20_000);
        assert!(config.show_untracked);
    }

    #[test]
    fn missing_explicit_config_path_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.toml");
        let err = load(Some(&path)).unwrap_err();
        let err = format!("{err:#}");
        assert!(
            err.contains("does-not-exist.toml"),
            "error must carry context naming the path, got: {err}"
        );
    }

    #[test]
    fn unknown_editor_default_profile_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(
            dir.path(),
            "bad-default.toml",
            r#"
[editor]
default = "not-installed"

[editor.profiles.nvim]
command = ["nvim", "+{line}", "{file}"]
"#,
        );
        let err = load(Some(&path)).unwrap_err();
        let err = format!("{err:#}");
        assert!(
            err.contains("not-installed"),
            "error must name the missing profile, got: {err}"
        );
    }

    #[test]
    fn malformed_toml_is_error_with_context() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(
            dir.path(),
            "malformed.toml",
            "refresh_debounce_ms = [1, 2\n",
        );
        let err = load(Some(&path)).unwrap_err();
        let err = format!("{err:#}");
        assert!(
            err.contains("malformed.toml"),
            "error must carry context naming the path, got: {err}"
        );
    }
}
