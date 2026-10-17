//! The setup wizard's Hyprland keybinding step

use std::path::PathBuf;

/// The shortcut the wizard offers, as shown to the user. Super+Shift+J is
/// left for the shell plugin's window
pub const SHORTCUT: &str = "Super+Alt+J";

/// Omarchy 4 configures Hyprland in Lua; older setups use hyprlang
const HYPRLAND_BIND_LUA: &str = r#"o.bind("SUPER + ALT + J", "Calendarchy", { tui = "calendarchy", focus = true })"#;
const HYPRLAND_BIND_CONF: &str = "bindd = SUPER ALT, J, Calendarchy, exec, omarchy-launch-or-focus-tui calendarchy";

/// The bindings file to add to and the line to add: `bindings.lua` when it
/// exists, else `bindings.conf`
fn hyprland_bindings() -> (PathBuf, &'static str) {
    let dir = dirs::home_dir().unwrap().join(".config/hypr");
    let lua = dir.join("bindings.lua");
    if lua.exists() { (lua, HYPRLAND_BIND_LUA) } else { (dir.join("bindings.conf"), HYPRLAND_BIND_CONF) }
}

/// Where the setup step adds the keybinding, for the wizard to show
pub fn hyprland_bindings_file() -> String {
    let (path, _) = hyprland_bindings();
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn hyprland_bind_exists() -> bool {
    std::fs::read_to_string(hyprland_bindings().0)
        .map(|content| has_tui_bind(&content))
        .unwrap_or(false)
}

/// A line that launches calendarchy, other than the shell window's toggle
fn has_tui_bind(content: &str) -> bool {
    content.lines().any(|line| line.contains("calendarchy") && !line.contains("calendarchy.window"))
}

pub fn install_shortcut() -> Result<(), String> {
    let (path, bind) = hyprland_bindings();
    let mut content = if path.exists() {
        std::fs::read_to_string(&path).map_err(|e| e.to_string())?
    } else {
        String::new()
    };
    if !content.ends_with('\n') && !content.is_empty() {
        content.push('\n');
    }
    content.push_str(bind);
    content.push('\n');
    std::fs::write(&path, content).map_err(|e| e.to_string())?;
    Ok(())
}

/// Returns true if the shortcut setup step should be shown in the wizard:
/// Hyprland is configured and has no binding that launches the TUI yet
pub fn should_show_shortcut_step() -> bool {
    dirs::home_dir()
        .map(|h| h.join(".config/hypr").exists())
        .unwrap_or(false)
        && !hyprland_bind_exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_window_toggle_is_not_a_tui_bind() {
        let window = r#"o.bind("SUPER + SHIFT + J", "Calendarchy window", "omarchy-shell shell toggle local.calendarchy.window '{}'")"#;
        assert!(!has_tui_bind(window));
        assert!(has_tui_bind(&format!("{window}\n{HYPRLAND_BIND_LUA}")));
        assert!(has_tui_bind(HYPRLAND_BIND_CONF));
        assert!(has_tui_bind(r#"o.bind("SUPER + ALT + J", "Calendarchy", { tui = "/home/me/.local/bin/calendarchy", focus = true })"#));
    }
}
