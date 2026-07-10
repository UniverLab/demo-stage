//! Interactive directory browser for picking local browser-pane files.

use std::path::{Path, PathBuf};

use inquire::Select;

use crate::error::{Error, Result};
use crate::paths::is_supported_browser_file;

/// Roots published by a running `demo capture`.
pub struct BrowseRoots {
    pub launch_dir: PathBuf,
    pub shell_dir: PathBuf,
}

enum Pick {
    Cancel,
    Parent,
    Goto(PathBuf),
    Enter(PathBuf),
    Select(PathBuf),
}

/// Browse directories and pick a supported local file (PDF, PNG, HTML, …).
pub fn pick_local_file(roots: &BrowseRoots, in_session: bool) -> Result<PathBuf> {
    let start = if in_session {
        roots.shell_dir.clone()
    } else {
        roots.launch_dir.clone()
    };
    browse(start, roots)
}

fn browse(mut current: PathBuf, roots: &BrowseRoots) -> Result<PathBuf> {
    loop {
        current = std::fs::canonicalize(&current).unwrap_or(current);

        println!("\n  {}\n", current.display());

        let mut options: Vec<(String, Pick)> = Vec::new();

        if let Some(parent) = current.parent() {
            if parent != current {
                options.push(("[..]  parent directory".into(), Pick::Parent));
            }
        }
        if current != roots.shell_dir {
            options.push((
                format!("[shell]  {}", roots.shell_dir.display()),
                Pick::Goto(roots.shell_dir.clone()),
            ));
        }
        if current != roots.launch_dir {
            options.push((
                format!("[demo]   {}", roots.launch_dir.display()),
                Pick::Goto(roots.launch_dir.clone()),
            ));
        }

        options.extend(list_entries(&current)?);
        options.push(("[cancel]".into(), Pick::Cancel));

        let labels: Vec<String> = options.iter().map(|(l, _)| l.clone()).collect();
        let choice = Select::new("Select:", labels.clone())
            .prompt()
            .map_err(|e| Error::Export(format!("file picker: {e}")))?;
        let idx = labels
            .iter()
            .position(|l| l == &choice)
            .unwrap_or(labels.len().saturating_sub(1));

        match &options[idx].1 {
            Pick::Cancel => {
                return Err(Error::Export("file selection cancelled".to_string()));
            }
            Pick::Parent => {
                current = current
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| roots.launch_dir.clone());
            }
            Pick::Goto(dir) | Pick::Enter(dir) => current = dir.clone(),
            Pick::Select(path) => return Ok(path.clone()),
        }
    }
}

fn list_entries(dir: &Path) -> Result<Vec<(String, Pick)>> {
    let read = std::fs::read_dir(dir)
        .map_err(|e| Error::Export(format!("cannot read {}: {e}", dir.display())))?;
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for ent in read.flatten() {
        let path = ent.path();
        let name = ent.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            dirs.push((format!("{name}/"), Pick::Enter(path)));
        } else if is_supported_browser_file(&path) {
            files.push((name, Pick::Select(path)));
        }
    }
    dirs.sort_by(|a, b| a.0.cmp(&b.0));
    files.sort_by(|a, b| a.0.cmp(&b.0));
    dirs.extend(files);
    Ok(dirs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn list_entries_filters_to_dirs_and_supported_files() {
        let dir = std::env::temp_dir().join(format!("demo-picker-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("nested")).unwrap();
        fs::write(dir.join("a.pdf"), b"x").unwrap();
        fs::write(dir.join("notes.txt"), b"x").unwrap();

        let entries = list_entries(&dir).unwrap();
        let labels: Vec<_> = entries.iter().map(|(l, _)| l.as_str()).collect();
        assert!(labels.iter().any(|l| l.starts_with("nested")));
        assert!(labels.contains(&"a.pdf"));
        assert!(!labels.contains(&"notes.txt"));
    }
}
