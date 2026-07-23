//! `demo doctor` — check the environment for the optional dependencies that
//! browser scenes (`demo open`) and the `mp4` target need, and report exactly how
//! to fix what's missing on this platform. `--fix` installs them where it can.
//!
//! The core pipeline (`capture` → `record` → `export gif`) needs none of this —
//! it's pure Rust. Chromium is only for browser panes, ffmpeg only for `mp4`.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::cli::DoctorArgs;
use crate::error::{Error, Result};
use crate::export::provision;

/// Severity of a single check's result.
enum Level {
    Ok,
    Warn,
    Fail,
}

struct Check {
    name: &'static str,
    level: Level,
    detail: String,
    /// A shell command the user can run to fix it.
    fix: Option<String>,
}

pub fn run(args: DoctorArgs) -> Result<()> {
    let plat = Platform::detect();
    println!("demo doctor — {}\n", plat.label());

    let chromium = check_chromium(&plat);
    let ffmpeg = check_ffmpeg(&plat);
    let display = check_display(&plat);

    for c in [&chromium, &ffmpeg, &display] {
        let (mark, _) = match c.level {
            Level::Ok => ("✓", ()),
            Level::Warn => ("⚠", ()),
            Level::Fail => ("✗", ()),
        };
        println!("{mark} {:<10} {}", c.name, c.detail);
        if let Some(fix) = &c.fix {
            println!("    fix: {fix}");
        }
    }

    // Summarise + optionally act.
    let needs_fix: Vec<&Check> = [&chromium, &ffmpeg]
        .into_iter()
        .filter(|c| matches!(c.level, Level::Fail))
        .collect();

    if args.fix {
        if needs_fix.is_empty() {
            println!("\nNothing to install — you're set.");
        } else if plat.apt {
            // The browser is the common blocker on WSL/Debian: install a non-snap
            // Google Chrome (preferred automatically over the snap).
            if matches!(chromium.level, Level::Fail) {
                println!("\n→ installing a non-snap Google Chrome…");
                install_chrome_apt()?;
            }
            if matches!(ffmpeg.level, Level::Fail) {
                println!("\n→ installing ffmpeg…");
                run_shell("sudo apt-get install -y ffmpeg")?;
            }
            println!("\nDone — re-run `demo doctor` to confirm.");
        } else {
            println!("\n--fix only automates apt-based Linux. Run the `fix:` commands above for your platform.");
        }
    } else if !needs_fix.is_empty() {
        println!(
            "\nSome optional deps are missing. Run `demo doctor --fix` (Linux/apt) \
             or the `fix:` commands above. The gif pipeline works without them."
        );
    } else {
        println!("\nAll set — browser scenes and mp4 will work.");
    }
    Ok(())
}

/// The host platform, enough to tailor the advice.
struct Platform {
    os: Os,
    wsl: bool,
    apt: bool,
}

enum Os {
    Linux,
    Mac,
    Windows,
    Other,
}

impl Platform {
    fn detect() -> Self {
        let os = if cfg!(target_os = "linux") {
            Os::Linux
        } else if cfg!(target_os = "macos") {
            Os::Mac
        } else if cfg!(target_os = "windows") {
            Os::Windows
        } else {
            Os::Other
        };
        let wsl = std::fs::read_to_string("/proc/version")
            .map(|v| v.to_lowercase().contains("microsoft"))
            .unwrap_or(false)
            || Path::new("/mnt/wslg").exists();
        let apt = matches!(os, Os::Linux) && on_path("apt-get").is_some();
        Platform { os, wsl, apt }
    }

    fn label(&self) -> String {
        let base = match self.os {
            Os::Linux => "Linux",
            Os::Mac => "macOS",
            Os::Windows => "Windows",
            Os::Other => "this platform",
        };
        if self.wsl {
            format!("{base} (WSL)")
        } else {
            base.to_string()
        }
    }
}

/// Browser scenes (`demo open`) need a working Chromium/Chrome that the automation
/// can drive — notably NOT the Ubuntu snap (its sandbox blocks the debug port).
fn check_chromium(plat: &Platform) -> Check {
    match provision::find_chromium() {
        Some(path) if is_snap(&path) => Check {
            name: "chromium",
            level: Level::Fail,
            detail: format!(
                "found the SNAP chromium ({}) — its sandbox blocks the debug port, \
                 so automation can't use it",
                path.display()
            ),
            fix: Some(chrome_install_hint(plat)),
        },
        Some(path) => Check {
            name: "chromium",
            level: Level::Ok,
            detail: format!("{}", path.display()),
            fix: None,
        },
        None => Check {
            name: "chromium",
            level: Level::Fail,
            detail: "not found — needed for `demo open` browser scenes".to_string(),
            fix: Some(chrome_install_hint(plat)),
        },
    }
}

/// `mp4` needs ffmpeg; it auto-downloads a managed copy on first use, so a missing
/// system ffmpeg is only a warning (a system install just avoids the download).
fn check_ffmpeg(plat: &Platform) -> Check {
    if ffmpeg_sidecar::command::ffmpeg_is_installed() {
        Check {
            name: "ffmpeg",
            level: Level::Ok,
            detail: "available (for the mp4 target)".to_string(),
            fix: None,
        }
    } else {
        Check {
            name: "ffmpeg",
            level: Level::Warn,
            detail: "not on PATH — `mp4` will auto-download a managed copy on first use"
                .to_string(),
            fix: Some(match plat.os {
                Os::Linux => "sudo apt-get install -y ffmpeg".to_string(),
                Os::Mac => "brew install ffmpeg".to_string(),
                _ => "install ffmpeg, or let `demo export mp4` fetch it".to_string(),
            }),
        }
    }
}

/// `demo open --view` (headed browser) needs a graphical display; everything else
/// is headless. Informational — not a failure.
fn check_display(plat: &Platform) -> Check {
    let has_display = matches!(plat.os, Os::Mac)
        || std::env::var_os("DISPLAY").is_some_and(|v| !v.is_empty())
        || std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty());
    if has_display {
        Check {
            name: "display",
            level: Level::Ok,
            detail: "a graphical display is available (`demo open --view` can run)".to_string(),
            fix: None,
        }
    } else {
        Check {
            name: "display",
            level: Level::Warn,
            detail: format!(
                "no graphical display — `demo open --view` needs one{}. Headless \
                 reveals still work.",
                if plat.wsl { " (on WSL: WSLg)" } else { "" }
            ),
            fix: None,
        }
    }
}

/// The platform-specific command to install a working (non-snap) browser.
fn chrome_install_hint(plat: &Platform) -> String {
    match plat.os {
        Os::Linux => "wget -O /tmp/google-chrome.deb https://dl.google.com/linux/direct/\
             google-chrome-stable_current_amd64.deb && sudo dpkg -i /tmp/google-chrome.deb \
             || sudo apt-get -f install -y     (or run `demo doctor --fix`)"
            .to_string(),
        Os::Mac => "brew install --cask google-chrome".to_string(),
        Os::Windows => "install Google Chrome from google.com/chrome".to_string(),
        Os::Other => "install Google Chrome or a non-snap Chromium".to_string(),
    }
}

/// Download and install the non-snap Google Chrome `.deb` (apt systems).
fn install_chrome_apt() -> Result<()> {
    run_shell(
        "set -e; cd /tmp; \
         wget -O google-chrome.deb https://dl.google.com/linux/direct/google-chrome-stable_current_amd64.deb; \
         sudo dpkg -i google-chrome.deb || sudo apt-get -f install -y",
    )
}

/// Run a shell command, inheriting the terminal so `sudo`/`wget` can prompt/print.
fn run_shell(script: &str) -> Result<()> {
    let status = Command::new("bash")
        .arg("-c")
        .arg(script)
        .status()
        .map_err(|e| Error::Export(format!("running `{script}`: {e}")))?;
    if !status.success() {
        return Err(Error::Export(format!(
            "command failed (exit {}): {script}",
            status.code().unwrap_or(-1)
        )));
    }
    Ok(())
}

/// Is this Chromium path the snap (its sandbox can't drive headless)? Detects a
/// `/snap/` path or a launcher that resolves to `snap`.
fn is_snap(path: &Path) -> bool {
    let under_snap = |p: &Path| p.components().any(|c| c.as_os_str() == "snap");
    if under_snap(path) {
        return true;
    }
    if let Ok(real) = std::fs::canonicalize(path) {
        return under_snap(&real) || real.file_name().is_some_and(|n| n == "snap");
    }
    false
}

fn on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_snap_detects_snap_path() {
        assert!(is_snap(Path::new("/snap/chromium/current/chromium")));
    }

    #[test]
    fn is_snap_rejects_non_snap_path() {
        assert!(!is_snap(Path::new("/usr/bin/chromium")));
        assert!(!is_snap(Path::new("/opt/google/chrome/chrome")));
    }

    #[test]
    fn is_snap_rejects_empty_path() {
        assert!(!is_snap(Path::new("")));
    }

    #[test]
    fn on_path_finds_existing_binary() {
        let result = on_path("ls");
        assert!(result.is_some());
        assert!(result.unwrap().is_file());
    }

    #[test]
    fn on_path_returns_none_for_nonexistent() {
        assert!(on_path("definitely_not_a_real_binary_xyz123").is_none());
    }

    #[test]
    fn chrome_install_hint_linux_mentions_deb() {
        let plat = Platform {
            os: Os::Linux,
            wsl: false,
            apt: true,
        };
        let hint = chrome_install_hint(&plat);
        assert!(hint.contains("google-chrome"));
        assert!(hint.contains("apt") || hint.contains("dpkg"));
    }

    #[test]
    fn chrome_install_hint_mac_mentions_brew() {
        let plat = Platform {
            os: Os::Mac,
            wsl: false,
            apt: false,
        };
        let hint = chrome_install_hint(&plat);
        assert!(hint.contains("brew"));
    }

    #[test]
    fn chrome_install_hint_windows_mentions_google() {
        let plat = Platform {
            os: Os::Windows,
            wsl: false,
            apt: false,
        };
        let hint = chrome_install_hint(&plat);
        assert!(hint.contains("google.com"));
    }

    #[test]
    fn chrome_install_hint_other_mentions_chrome() {
        let plat = Platform {
            os: Os::Other,
            wsl: false,
            apt: false,
        };
        let hint = chrome_install_hint(&plat);
        assert!(hint.contains("Chrome") || hint.contains("Chromium"));
    }

    #[test]
    fn platform_label_linux() {
        let plat = Platform {
            os: Os::Linux,
            wsl: false,
            apt: true,
        };
        assert_eq!(plat.label(), "Linux");
    }

    #[test]
    fn platform_label_wsl() {
        let plat = Platform {
            os: Os::Linux,
            wsl: true,
            apt: true,
        };
        assert_eq!(plat.label(), "Linux (WSL)");
    }

    #[test]
    fn platform_label_mac() {
        let plat = Platform {
            os: Os::Mac,
            wsl: false,
            apt: false,
        };
        assert_eq!(plat.label(), "macOS");
    }

    #[test]
    fn platform_label_windows() {
        let plat = Platform {
            os: Os::Windows,
            wsl: false,
            apt: false,
        };
        assert_eq!(plat.label(), "Windows");
    }

    #[test]
    fn platform_label_other() {
        let plat = Platform {
            os: Os::Other,
            wsl: false,
            apt: false,
        };
        assert_eq!(plat.label(), "this platform");
    }

    #[test]
    fn check_display_ok_when_display_set() {
        let plat = Platform {
            os: Os::Linux,
            wsl: false,
            apt: false,
        };
        let original = std::env::var_os("DISPLAY");
        let original_w = std::env::var_os("WAYLAND_DISPLAY");
        std::env::set_var("DISPLAY", ":0");
        std::env::remove_var("WAYLAND_DISPLAY");
        let check = check_display(&plat);
        assert!(matches!(check.level, Level::Ok));
        if let Some(v) = original { std::env::set_var("DISPLAY", v) } else { std::env::remove_var("DISPLAY") }
        if let Some(v) = original_w { std::env::set_var("WAYLAND_DISPLAY", v) }
    }

    #[test]
    fn check_display_warn_when_no_display() {
        let plat = Platform {
            os: Os::Linux,
            wsl: false,
            apt: false,
        };
        let original = std::env::var_os("DISPLAY");
        let original_w = std::env::var_os("WAYLAND_DISPLAY");
        std::env::remove_var("DISPLAY");
        std::env::remove_var("WAYLAND_DISPLAY");
        let check = check_display(&plat);
        assert!(matches!(check.level, Level::Warn));
        if let Some(v) = original { std::env::set_var("DISPLAY", v) }
        if let Some(v) = original_w { std::env::set_var("WAYLAND_DISPLAY", v) }
    }

    #[test]
    fn check_ffmpeg_reports_status() {
        let plat = Platform {
            os: Os::Linux,
            wsl: false,
            apt: true,
        };
        let check = check_ffmpeg(&plat);
        // Just verify it returns without panic
        assert!(!check.name.is_empty());
        assert!(!check.detail.is_empty());
    }

    #[test]
    fn check_chromium_returns_check() {
        let plat = Platform {
            os: Os::Linux,
            wsl: false,
            apt: true,
        };
        let check = check_chromium(&plat);
        assert_eq!(check.name, "chromium");
        assert!(!check.detail.is_empty());
    }
}
