//! Where Z-Line keeps its session, thread cache and SQLite history.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

/// Profile names become directory names, so keep them to a safe alphabet.
pub fn validate_profile(name: &str) -> Result<()> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("--profile chỉ chấp nhận chữ, số, '-' hoặc '_' (nhận được {name:?})");
    }
    Ok(())
}

/// Pure path logic, separated from the environment so it can be unit-tested.
///
/// An explicit `--data-dir` wins (relative paths are anchored at `cwd`); otherwise the directory is
/// `<data_home>/z-line/profiles/<profile>`.
pub fn data_dir_for(
    explicit: Option<&Path>,
    profile: &str,
    cwd: &Path,
    data_home: &Path,
) -> Result<PathBuf> {
    if let Some(p) = explicit {
        if p.as_os_str().is_empty() {
            bail!("--data-dir không được để trống");
        }
        return Ok(if p.is_absolute() {
            p.to_path_buf()
        } else {
            cwd.join(p)
        });
    }
    validate_profile(profile)?;
    Ok(data_home.join("z-line").join("profiles").join(profile))
}

/// Per-user data home: `$XDG_DATA_HOME`, `%APPDATA%`, or the platform default under `$HOME`.
pub fn data_home() -> Result<PathBuf> {
    if let Some(x) = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        return Ok(x);
    }
    if cfg!(windows) {
        if let Some(a) = std::env::var_os("APPDATA") {
            return Ok(PathBuf::from(a));
        }
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .context("Không xác định được thư mục HOME; hãy dùng --data-dir")?;
    let rel = if cfg!(target_os = "macos") {
        "Library/Application Support"
    } else {
        ".local/share"
    };
    Ok(home.join(rel))
}

/// Resolve the data directory for the current process.
pub fn resolve(explicit: Option<&Path>, profile: &str) -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("Không xác định được thư mục làm việc")?;
    match explicit {
        Some(_) => data_dir_for(explicit, profile, &cwd, Path::new("")),
        None => data_dir_for(None, profile, &cwd, &data_home()?),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_goes_under_profiles() {
        let p = data_dir_for(None, "work", Path::new("/cwd"), Path::new("/home/u/.local/share")).unwrap();
        assert_eq!(p, Path::new("/home/u/.local/share/z-line/profiles/work"));
    }

    #[test]
    fn explicit_relative_is_anchored_at_cwd() {
        let p = data_dir_for(Some(Path::new("data")), "x", Path::new("/cwd"), Path::new("/h")).unwrap();
        assert_eq!(p, Path::new("/cwd/data"));
    }

    #[test]
    fn explicit_absolute_is_kept() {
        let p = data_dir_for(Some(Path::new("/srv/z")), "x", Path::new("/cwd"), Path::new("/h")).unwrap();
        assert_eq!(p, Path::new("/srv/z"));
    }

    #[test]
    fn rejects_bad_profile_names() {
        for bad in ["", "../x", "a/b", "a b", "tên"] {
            assert!(validate_profile(bad).is_err(), "{bad:?} should be rejected");
        }
        assert!(validate_profile("work-2_a").is_ok());
    }

    #[test]
    fn rejects_empty_data_dir() {
        assert!(data_dir_for(Some(Path::new("")), "x", Path::new("/c"), Path::new("/h")).is_err());
    }
}
