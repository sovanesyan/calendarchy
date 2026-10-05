use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::process::Command;

// === macOS hotkey setup ===

#[cfg(target_os = "macos")]
const PLIST_LABEL: &str = "com.calendarchy.hotkey";

#[cfg(target_os = "macos")]
pub struct Terminal {
    pub name: &'static str,
    app: &'static str,
    launch_fmt: &'static str,
}

#[cfg(target_os = "macos")]
const TERMINALS: &[Terminal] = &[
    Terminal { name: "Ghostty", app: "Ghostty.app", launch_fmt: "open -na Ghostty.app --args --window-width=120 --window-height=40 -e {}" },
    Terminal { name: "iTerm2", app: "iTerm.app", launch_fmt: "open -na iTerm.app --args {}" },
    Terminal { name: "Alacritty", app: "Alacritty.app", launch_fmt: "open -na Alacritty.app --args --option window.dimensions.columns=120 --option window.dimensions.lines=40 -e {}" },
    Terminal { name: "Kitty", app: "kitty.app", launch_fmt: "open -na kitty.app --args -o initial_window_width=120c -o initial_window_height=40c {}" },
    Terminal { name: "WezTerm", app: "WezTerm.app", launch_fmt: "open -na WezTerm.app --args --config initial_cols=120 --config initial_rows=40 start -- {}" },
    Terminal { name: "Terminal", app: "Terminal.app", launch_fmt: "osascript -e 'tell application \"Terminal\"' -e 'do script \"{}\"' -e 'set number of columns of front window to 120' -e 'set number of rows of front window to 40' -e 'activate' -e 'end tell'" },
];

#[cfg(target_os = "macos")]
pub fn detect_terminal_names() -> Vec<String> {
    TERMINALS.iter().filter(|t| {
        PathBuf::from(format!("/Applications/{}", t.app)).exists()
            || PathBuf::from(format!("/System/Applications/{}", t.app)).exists()
            || PathBuf::from(format!("/System/Applications/Utilities/{}", t.app)).exists()
    }).map(|t| t.name.to_string()).collect()
}

#[cfg(target_os = "macos")]
fn find_hotkey_binary() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe.parent().unwrap().join("calendarchy-hotkey");
        if sibling.exists() {
            return Some(sibling);
        }
    }
    if let Ok(output) = Command::new("which").arg("calendarchy-hotkey").output() {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Some(PathBuf::from(path));
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn plist_path() -> PathBuf {
    dirs::home_dir().unwrap()
        .join("Library/LaunchAgents")
        .join(format!("{}.plist", PLIST_LABEL))
}

#[cfg(target_os = "macos")]
fn calendarchy_path() -> String {
    std::env::current_exe()
        .unwrap_or_else(|_| PathBuf::from("/usr/local/bin/calendarchy"))
        .to_string_lossy()
        .to_string()
}

#[cfg(target_os = "macos")]
pub fn install_shortcut(terminal_index: usize) -> Result<(), String> {
    let hotkey_bin = find_hotkey_binary()
        .ok_or("calendarchy-hotkey not found. Build it with: swiftc -O swift/main.swift -o calendarchy-hotkey, and put it next to calendarchy")?;

    let terminals: Vec<&Terminal> = TERMINALS.iter().filter(|t| {
        PathBuf::from(format!("/Applications/{}", t.app)).exists()
            || PathBuf::from(format!("/System/Applications/{}", t.app)).exists()
            || PathBuf::from(format!("/System/Applications/Utilities/{}", t.app)).exists()
    }).collect();

    let terminal = terminals.get(terminal_index)
        .ok_or("Invalid terminal selection")?;

    let cal_path = calendarchy_path();
    let launch_cmd = terminal.launch_fmt.replace("{}", &cal_path);

    let plist_dir = dirs::home_dir().unwrap().join("Library/LaunchAgents");
    std::fs::create_dir_all(&plist_dir).map_err(|e| e.to_string())?;

    let plist_content = format!(
r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{binary}</string>
        <string>{command}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
</dict>
</plist>"#,
        label = PLIST_LABEL,
        binary = hotkey_bin.display(),
        command = launch_cmd,
    );

    let plist = plist_path();
    if plist.exists() {
        let _ = Command::new("launchctl").args(["unload", &plist.to_string_lossy()]).output();
    }

    std::fs::write(&plist, &plist_content).map_err(|e| e.to_string())?;

    let output = Command::new("launchctl")
        .args(["load", &plist.to_string_lossy()])
        .output()
        .map_err(|e| e.to_string())?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Failed to load agent: {}", err));
    }

    Ok(())
}

#[cfg(target_os = "macos")]
pub fn remove_setup() -> Result<(), Box<dyn std::error::Error>> {
    let plist = plist_path();

    if !plist.exists() {
        println!("No hotkey agent found. Nothing to remove.");
        return Ok(());
    }

    let _ = Command::new("launchctl")
        .args(["unload", &plist.to_string_lossy()])
        .output();

    std::fs::remove_file(&plist)?;
    println!("Hotkey agent removed.");
    Ok(())
}

// === Linux Hyprland setup ===

#[cfg(target_os = "linux")]
const HYPRLAND_BIND: &str = "bindd = SUPER SHIFT, J, Calendarchy, exec, omarchy-launch-tui calendarchy";

#[cfg(target_os = "linux")]
fn hyprland_bindings_path() -> PathBuf {
    dirs::home_dir().unwrap().join(".config/hypr/bindings.conf")
}

#[cfg(target_os = "linux")]
fn hyprland_bind_exists() -> bool {
    let path = hyprland_bindings_path();
    if !path.exists() { return false; }
    std::fs::read_to_string(&path)
        .map(|content| content.contains("calendarchy"))
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
pub fn install_shortcut() -> Result<(), String> {
    let path = hyprland_bindings_path();
    let mut content = if path.exists() {
        std::fs::read_to_string(&path).map_err(|e| e.to_string())?
    } else {
        String::new()
    };
    if !content.ends_with('\n') && !content.is_empty() {
        content.push('\n');
    }
    content.push_str(HYPRLAND_BIND);
    content.push('\n');
    std::fs::write(&path, content).map_err(|e| e.to_string())?;
    Ok(())
}

// === Platform-agnostic ===

/// Returns true if the shortcut setup step should be shown in the wizard
pub fn should_show_shortcut_step() -> bool {
    #[cfg(target_os = "macos")]
    {
        !plist_path().exists() && find_hotkey_binary().is_some()
    }

    #[cfg(target_os = "linux")]
    {
        dirs::home_dir()
            .map(|h| h.join(".config/hypr").exists())
            .unwrap_or(false)
            && !hyprland_bind_exists()
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    { false }
}
