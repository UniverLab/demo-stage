//! `demo doctor` — check the environment for the optional dependencies that
//! browser scenes (`demo open`) and the `mp4` target need, and report exactly how
//! to fix what's missing on this platform. `--fix` installs them where it can.
//!
//! The core pipeline (`capture` → `record` → `export gif`) needs none of this —
//! it's pure Rust. Chromium is only for browser panes, ffmpeg only for `mp4`.

use std::io::{IsTerminal, Write};
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

    let mut browser_just_installed = false;

    if args.fix {
        if needs_fix.is_empty() {
            println!("\nNothing to install — you're set.");
        } else if plat.apt {
            // The browser is the common blocker on WSL/Debian: install a non-snap
            // Google Chrome (preferred automatically over the snap).
            if matches!(chromium.level, Level::Fail) {
                println!("\n→ installing a non-snap Google Chrome…");
                install_chrome_apt()?;
                browser_just_installed = true;
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

    // WSL browser routing: after install or on explicit request.
    if plat.wsl && (browser_just_installed || args.route_browser) {
        handle_wsl_browser_routing()?;
    }

    Ok(())
}

/// Returns true if the contents of `/proc/version` indicate we are running
/// under WSL (the string contains "microsoft", case-insensitive).
fn proc_version_indicates_wsl(contents: &str) -> bool {
    contents.to_lowercase().contains("microsoft")
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
            .map(|v| proc_version_indicates_wsl(&v))
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

/// WSL browser routing: after installing Chrome on WSL, the desktop defaults
/// get hijacked. Report the measured state and offer to route human-facing
/// links to the Windows browser instead.
fn handle_wsl_browser_routing() -> Result<()> {
    let env = WslBrowserEnv::from_home_or_temp();
    let state = WslBrowserState::detect();
    wsl_browser_routing_inner(
        &env,
        &state,
        std::io::stdin().is_terminal(),
        ask_route_prompt,
        register_desktop_default,
    )
}

/// Register a .desktop entry as the default for http/https/html schemes.
/// Injectable so tests can record calls without mutating the real desktop.
fn register_desktop_default(desktop_name: &str) -> Result<bool> {
    let mime_script = format!(
        "xdg-mime default {desktop_name} x-scheme-handler/http \
         x-scheme-handler/https text/html"
    );
    let (_, mime_ok) = run_silent(&mime_script)?;
    let settings_script = format!("xdg-settings set default-web-browser {desktop_name}");
    let (_, settings_ok) = run_silent(&settings_script)?;
    Ok(mime_ok && settings_ok)
}

/// The measured state of desktop browser defaults on WSL.
struct WslBrowserState {
    default_web_browser: String,
    scheme_handler_https: String,
}

impl WslBrowserState {
    fn detect() -> Self {
        let default_web_browser = run_silent("xdg-settings get default-web-browser")
            .map(|(s, _)| s)
            .unwrap_or_else(|_| "(unknown)".to_string());
        let scheme_handler_https = run_silent("xdg-mime query default x-scheme-handler/https")
            .map(|(s, _)| s)
            .unwrap_or_else(|_| "(unknown)".to_string());
        Self {
            default_web_browser,
            scheme_handler_https,
        }
    }

    fn points_at_chrome(&self) -> bool {
        let d = &self.default_web_browser;
        let s = &self.scheme_handler_https;
        d.contains("google-chrome") || s.contains("google-chrome")
    }
}

/// Paths and directories used by the WSL browser routing. Separated so tests
/// can point at a tempdir without touching the real home.
struct WslBrowserEnv {
    home: PathBuf,
    xdg_data_home: PathBuf,
    shell_profile: PathBuf,
}

impl WslBrowserEnv {
    fn from_home_or_temp() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let xdg_data_home = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"));
        let shell_profile = home.join(".bashrc");
        Self {
            home,
            xdg_data_home,
            shell_profile,
        }
    }

    fn launcher_path(&self) -> PathBuf {
        self.home.join(".local/bin/wsl-open-browser")
    }

    fn desktop_path(&self) -> PathBuf {
        self.xdg_data_home
            .join("applications/wsl-open-browser.desktop")
    }
}

/// Inner routing logic, parameterised by detected browser state, TTY-ness, a
/// prompt closure, and a registration closure so tests can exercise both paths
/// without a real terminal or touching the developer's real desktop config.
fn wsl_browser_routing_inner(
    env: &WslBrowserEnv,
    state: &WslBrowserState,
    is_tty: bool,
    prompt: impl FnOnce() -> bool,
    register_default: impl FnOnce(&str) -> Result<bool>,
) -> Result<()> {
    println!("\n→ WSL browser routing:");
    println!(
        "    xdg-settings get default-web-browser = {}",
        state.default_web_browser.trim()
    );
    println!(
        "    xdg-mime query default x-scheme-handler/https = {}",
        state.scheme_handler_https.trim()
    );

    if !state.points_at_chrome() {
        println!("    (default does not point at the installed Chrome — no routing needed)");
        return Ok(());
    }

    println!(
        "\n    The installed Chrome just registered itself as the default for http/https.\n\
         That means `gh auth login`, `xdg-open`, and anything reading the desktop\n\
         database will open links in a browser that has no profile and no mouse\n\
         under WSL. An OAuth flow that opens there is a dead end."
    );

    let do_route = if !is_tty {
        println!(
            "\n    (non-interactive: not prompting — run `demo doctor --route-browser` to fix)"
        );
        print_routing_commands(env);
        return Ok(());
    } else {
        prompt()
    };

    if do_route {
        apply_wsl_browser_routing(env, state, register_default)?;
    } else {
        println!("\n    Left desktop defaults unchanged. To route later:");
        print_routing_commands(env);
    }

    Ok(())
}

/// Ask the user whether to route human-facing links to the Windows browser.
/// Default is yes.
fn ask_route_prompt() -> bool {
    inquire::Confirm::new("Route human-facing links (http/https) to the Windows browser?")
        .with_default(true)
        .prompt()
        .unwrap_or_default()
}

/// Write the launcher, .desktop entry, and BROWSER export. Idempotent.
fn apply_wsl_browser_routing(
    env: &WslBrowserEnv,
    state: &WslBrowserState,
    register_default: impl FnOnce(&str) -> Result<bool>,
) -> Result<()> {
    let launcher = env.launcher_path();
    let desktop = env.desktop_path();
    let profile = &env.shell_profile;

    // Write launcher script.
    if let Some(parent) = launcher.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Error::Export(format!("creating {}: {e}", parent.display())))?;
    }
    std::fs::write(&launcher, launcher_script())
        .map_err(|e| Error::Export(format!("writing {}: {e}", launcher.display())))?;
    make_executable(&launcher)?;

    // Write .desktop entry with absolute Exec path.
    if let Some(parent) = desktop.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Error::Export(format!("creating {}: {e}", parent.display())))?;
    }
    std::fs::write(&desktop, desktop_entry(&launcher))
        .map_err(|e| Error::Export(format!("writing {}: {e}", desktop.display())))?;

    // Register as default for http/https/html (injectable for tests).
    let desktop_name = "wsl-open-browser.desktop";
    let all_ok = register_default(desktop_name)?;

    // Add BROWSER export to shell profile (idempotent).
    let browser_line = format!("export BROWSER={}", launcher.display());
    append_line_idempotent(profile, &browser_line)?;

    println!("\n    Wrote:");
    println!("      {}", launcher.display());
    println!("      {}", desktop.display());
    println!("      {} (appended: {})", profile.display(), browser_line);
    if !all_ok {
        println!(
            "\n    Warning: xdg-mime/xdg-settings registration failed \
             (xdg-utils may not be installed). The .desktop file is written \
             but may not be the default handler."
        );
    }
    println!("\n    To undo:");
    print_undo_commands(env, state);

    Ok(())
}

/// The launcher script content. Uses PowerShell's Start-Process to pass the URL
/// as a single argument (cmd.exe /c start splits on &).
fn launcher_script() -> String {
    r#"#!/bin/sh
# Route a URL to the Windows-side default browser via PowerShell.
# Uses Start-Process so the URL is passed as a single argument — cmd.exe /c start
# splits on & and breaks OAuth callbacks.
exec powershell.exe -NoProfile -Command "Start-Process \"$1\""
"#
    .to_string()
}

/// The .desktop entry content. Uses the absolute launcher path in Exec so the
/// desktop-handler path works even if ~/.local/bin is not on $PATH.
fn desktop_entry(launcher_path: &Path) -> String {
    format!(
        r#"[Desktop Entry]
Version=1.0
Type=Application
Name=WSL Open Browser
Comment=Open URLs in the Windows-side default browser
Exec={} %u
Terminal=false
Categories=Network;WebBrowser;
MimeType=x-scheme-handler/http;x-scheme-handler/https;text/html;
"#,
        launcher_path.display()
    )
}

/// Make a file executable (chmod +x).
///
/// Unix only: the whole browser-routing feature is (it detects WSL by reading
/// `/proc/version` and registers a `.desktop` handler), but this helper was the
/// one piece that failed to *compile* elsewhere — `std::os::unix` does not exist
/// on Windows, and the release build only found out on the Windows runner, after
/// the tag had already been cut.
#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)
        .map_err(|e| Error::Export(format!("metadata {}: {e}", path.display())))?
        .permissions();
    let mode = perms.mode() | 0o111;
    perms.set_mode(mode);
    std::fs::set_permissions(path, perms)
        .map_err(|e| Error::Export(format!("chmod {}: {e}", path.display())))?;
    Ok(())
}

/// On Windows there is no execute bit to set, so this is a no-op that keeps the
/// callers compiling. The feature itself never runs there — `is_wsl()` is false.
#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

/// Append a line to a file if it is not already present. Idempotent.
fn append_line_idempotent(path: &Path, line: &str) -> Result<()> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    if existing.lines().any(|l| l.trim() == line.trim()) {
        return Ok(());
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| Error::Export(format!("opening {}: {e}", path.display())))?;
    writeln!(f, "{}", line)
        .map_err(|e| Error::Export(format!("writing {}: {e}", path.display())))?;
    Ok(())
}

/// Print the commands to route browser links to Windows.
fn print_routing_commands(env: &WslBrowserEnv) {
    println!("\n    To route manually:");
    println!(
        "      mkdir -p {}",
        env.launcher_path().parent().unwrap().display()
    );
    println!("      cat > {} <<'EOF'", env.launcher_path().display());
    println!("{}", launcher_script());
    println!("EOF");
    println!("      chmod +x {}", env.launcher_path().display());
    println!(
        "      mkdir -p {}",
        env.desktop_path().parent().unwrap().display()
    );
    println!("      cat > {} <<'EOF'", env.desktop_path().display());
    println!("{}", desktop_entry(&env.launcher_path()));
    println!("EOF");
    println!(
        "      xdg-mime default wsl-open-browser.desktop x-scheme-handler/http \
         x-scheme-handler/https text/html"
    );
    println!("      xdg-settings set default-web-browser wsl-open-browser.desktop");
    println!(
        "      echo 'export BROWSER={}' >> {}",
        env.launcher_path().display(),
        env.shell_profile.display()
    );
}

/// Print the commands to undo the routing. Uses the measured default browser
/// from state rather than a hard-coded name, so the undo restores what was
/// there before (e.g. google-chrome-stable.desktop).
fn print_undo_commands(env: &WslBrowserEnv, state: &WslBrowserState) {
    let previous = state.default_web_browser.trim();
    println!("      rm {}", env.launcher_path().display());
    println!("      rm {}", env.desktop_path().display());
    println!(
        "      xdg-mime default {previous} x-scheme-handler/http \
         x-scheme-handler/https text/html"
    );
    println!("      xdg-settings set default-web-browser {previous}");
    println!(
        "      sed -i '\\|export BROWSER={}|d' {}",
        env.launcher_path().display(),
        env.shell_profile.display()
    );
}

/// Run a command and capture its stdout (trimmed) plus whether it exited successfully.
fn run_silent(script: &str) -> Result<(String, bool)> {
    let output = Command::new("bash")
        .arg("-c")
        .arg(script)
        .output()
        .map_err(|e| Error::Export(format!("running `{script}`: {e}")))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((stdout, output.status.success()))
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
        if let Some(v) = original {
            std::env::set_var("DISPLAY", v)
        } else {
            std::env::remove_var("DISPLAY")
        }
        if let Some(v) = original_w {
            std::env::set_var("WAYLAND_DISPLAY", v)
        }
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
        if let Some(v) = original {
            std::env::set_var("DISPLAY", v)
        }
        if let Some(v) = original_w {
            std::env::set_var("WAYLAND_DISPLAY", v)
        }
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

    #[test]
    fn wsl_detection_microsoft_in_proc_version() {
        let content = "Linux version 5.15.0 (microsoft-standard-WSL2)";
        assert!(proc_version_indicates_wsl(content));
    }

    #[test]
    fn wsl_detection_plain_linux_no_microsoft() {
        let content = "Linux version 5.15.0-generic (builder@host)";
        assert!(!proc_version_indicates_wsl(content));
    }

    #[test]
    fn launcher_script_uses_start_process() {
        let script = launcher_script();
        assert!(script.contains("Start-Process"));
        assert!(script.contains("$1"));
    }

    #[test]
    fn append_line_idempotent_adds_once() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("profile");
        std::fs::write(&file, "existing line\n").unwrap();

        append_line_idempotent(&file, "export BROWSER=/tmp/test").unwrap();
        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(content, "existing line\nexport BROWSER=/tmp/test\n");

        // Second call should not add another line.
        append_line_idempotent(&file, "export BROWSER=/tmp/test").unwrap();
        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(content, "existing line\nexport BROWSER=/tmp/test\n");
    }

    #[test]
    fn wsl_browser_routing_non_tty_does_not_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let env = WslBrowserEnv {
            home: dir.path().to_path_buf(),
            xdg_data_home: dir.path().join(".local/share"),
            shell_profile: dir.path().join(".bashrc"),
        };
        std::fs::write(&env.shell_profile, "").unwrap();

        // Force the hijacked state so control reaches the !is_tty branch
        // (otherwise on a non-hijacked host the function returns early and
        // the test passes vacuously).
        let state = WslBrowserState {
            default_web_browser: "google-chrome.desktop".to_string(),
            scheme_handler_https: "google-chrome.desktop".to_string(),
        };

        let prompt_called = std::cell::Cell::new(false);
        let register_called = std::cell::Cell::new(false);
        let result = wsl_browser_routing_inner(
            &env,
            &state,
            false,
            || {
                prompt_called.set(true);
                true
            },
            |_desktop_name| {
                register_called.set(true);
                Ok(true)
            },
        );
        assert!(result.is_ok());
        // The prompt closure must NOT have been called in non-TTY mode.
        assert!(!prompt_called.get());
        // Registration must NOT have been called either.
        assert!(!register_called.get());
        // No launcher or desktop file should have been created.
        assert!(!env.launcher_path().exists());
        assert!(!env.desktop_path().exists());
        // The profile file should be unchanged (empty).
        let profile_content = std::fs::read_to_string(&env.shell_profile).unwrap();
        assert_eq!(profile_content, "");
    }

    #[test]
    fn wsl_browser_routing_apply_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let env = WslBrowserEnv {
            home: dir.path().to_path_buf(),
            xdg_data_home: dir.path().join(".local/share"),
            shell_profile: dir.path().join(".bashrc"),
        };
        std::fs::write(&env.shell_profile, "").unwrap();

        let state = WslBrowserState {
            default_web_browser: "google-chrome.desktop".to_string(),
            scheme_handler_https: "google-chrome.desktop".to_string(),
        };

        // Record registration calls to verify they happen with the right args.
        let register_calls: std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(vec![]);

        // First invocation: TTY + prompt says yes → apply_wsl_browser_routing runs.
        let r1 = wsl_browser_routing_inner(
            &env,
            &state,
            true,
            || true,
            |desktop_name| {
                register_calls.borrow_mut().push(desktop_name.to_string());
                Ok(true)
            },
        );
        assert!(r1.is_ok());

        // Second invocation: same inputs → idempotent, no duplicates.
        let r2 = wsl_browser_routing_inner(
            &env,
            &state,
            true,
            || true,
            |desktop_name| {
                register_calls.borrow_mut().push(desktop_name.to_string());
                Ok(true)
            },
        );
        assert!(r2.is_ok());

        // Registration was called (twice, once per apply), with the right name.
        let calls = register_calls.borrow();
        assert_eq!(calls.len(), 2);
        assert!(calls.iter().all(|n| n == "wsl-open-browser.desktop"));

        // Exactly one launcher.
        assert!(env.launcher_path().exists());
        let launcher_content = std::fs::read_to_string(env.launcher_path()).unwrap();
        assert!(launcher_content.contains("Start-Process"));
        // The exec line (not the comment) must not use cmd.exe.
        let exec_line = launcher_content
            .lines()
            .find(|l| l.starts_with("exec "))
            .unwrap();
        assert!(!exec_line.contains("cmd.exe"));

        // Exactly one .desktop entry.
        assert!(env.desktop_path().exists());
        let desktop_content = std::fs::read_to_string(env.desktop_path()).unwrap();
        assert!(desktop_content.contains("[Desktop Entry]"));
        assert!(desktop_content.contains("wsl-open-browser"));
        // The Exec line must use the absolute launcher path.
        let exec_line_desktop = desktop_content
            .lines()
            .find(|l| l.starts_with("Exec="))
            .unwrap();
        assert!(exec_line_desktop.contains(&env.launcher_path().to_string_lossy().to_string()));

        // Exactly one BROWSER line in the profile.
        let profile_content = std::fs::read_to_string(&env.shell_profile).unwrap();
        let browser_lines: Vec<&str> = profile_content
            .lines()
            .filter(|l| l.contains("export BROWSER="))
            .collect();
        assert_eq!(
            browser_lines.len(),
            1,
            "expected exactly one BROWSER line, got: {browser_lines:?}"
        );
    }

    #[test]
    fn launcher_passes_url_with_ampersand_as_single_arg() {
        let dir = tempfile::tempdir().unwrap();
        let stub_dir = dir.path().join("bin");
        std::fs::create_dir_all(&stub_dir).unwrap();
        let log_file = dir.path().join("args.log");

        // Stub powershell.exe that logs its arguments, one per line.
        let stub = format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > {}", log_file.display());
        let stub_path = stub_dir.join("powershell.exe");
        std::fs::write(&stub_path, &stub).unwrap();
        make_executable(&stub_path).unwrap();

        // Write the real launcher to the tempdir.
        let launcher = dir.path().join("wsl-open-browser");
        std::fs::write(&launcher, launcher_script()).unwrap();
        make_executable(&launcher).unwrap();

        // Run the launcher directly (not through sh -c) with a URL containing &.
        // The URL must arrive as exactly one argv entry with & intact — that's
        // the case that motivated using Start-Process.
        let test_url = "https://example.com/cb?code=x&state=y";
        let status = std::process::Command::new(&launcher)
            .arg(test_url)
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    stub_dir.display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .status()
            .expect("failed to spawn launcher");
        assert!(status.success(), "launcher exited with {status}");

        let logged = std::fs::read_to_string(&log_file).unwrap();
        let args: Vec<&str> = logged.lines().collect();
        // Start-Process "$1" should produce args like:
        //   -NoProfile -Command Start-Process "<url>"
        // The URL itself must be ONE arg (not split on &).
        let joined = args.join(" ");
        assert!(
            joined.contains(test_url),
            "expected URL {test_url} intact in args: {args:?}"
        );
        // And the URL must NOT have been split — there should be no standalone
        // "state=y" arg without the leading "https://example.com/cb?code=x&".
        for arg in &args {
            if arg.contains("state=y") {
                assert!(
                    arg.contains("code=x"),
                    "URL was split on &: got arg {arg:?} in {args:?}"
                );
            }
        }
    }
}
