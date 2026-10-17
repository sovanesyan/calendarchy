//! What the terminal said its background is, remembered per terminal across
//! runs. A terminal that never answered is not asked again (that costs ~200ms
//! and its late reply could arrive as keystrokes); one that answered is asked
//! again once the first frame is up, in case the theme changed.

use std::path::PathBuf;
use std::time::Duration;

fn term_bg_path() -> Option<PathBuf> {
    dirs::cache_dir().map(|p| p.join("calendarchy").join("term-bg"))
}

/// What we learned about the terminal's background on a previous run
pub enum TermBg {
    Color(u8, u8, u8),
    Unanswered,
}

/// Which terminal we're in, as far as the environment tells: multiplexers and
/// ssh first (they decide whether OSC 11 gets answered), then the emulator
fn terminal_identity() -> String {
    let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    let mut parts = Vec::new();
    if var("TMUX").is_some() {
        parts.push("tmux".to_string());
    }
    if var("ZELLIJ").is_some() {
        parts.push("zellij".to_string());
    }
    if var("SSH_TTY").is_some() || var("SSH_CONNECTION").is_some() {
        parts.push("ssh".to_string());
    }
    parts.push(var("TERM_PROGRAM").or_else(|| var("TERM")).unwrap_or_else(|| "unknown".into()));
    // One token, so it can't be confused with the value on its line
    parts.join("+").replace(char::is_whitespace, "_")
}

/// The remembered answer for `terminal` in the term-bg file: one
/// "<terminal> <rrggbb|none>" line per terminal
fn parse_term_bg(text: &str, terminal: &str) -> Option<TermBg> {
    let value = text.lines().find_map(|line| {
        let (who, value) = line.trim().split_once(' ')?;
        (who == terminal).then_some(value.trim())
    })?;
    if value == "none" {
        return Some(TermBg::Unanswered);
    }
    if value.len() != 6 {
        return None;
    }
    let [_, r, g, b] = u32::from_str_radix(value, 16).ok()?.to_be_bytes();
    Some(TermBg::Color(r, g, b))
}

/// The term-bg file with `terminal`'s line set to `value`, others kept.
/// Lines from the old single-value format (no terminal name) are dropped.
fn update_term_bg(text: &str, terminal: &str, value: &str) -> String {
    let mut out: String = text
        .lines()
        .filter(|line| line.trim().split_once(' ').is_some_and(|(who, _)| who != terminal))
        .map(|line| format!("{}\n", line.trim()))
        .collect();
    out.push_str(&format!("{} {}\n", terminal, value));
    out
}

/// What this terminal answered last time, if it was ever asked
pub fn load() -> Option<TermBg> {
    let text = std::fs::read_to_string(term_bg_path()?).ok()?;
    parse_term_bg(&text, &terminal_identity())
}

/// Ask the terminal for its background via OSC 11, remembering the outcome
pub fn probe() -> Option<(u8, u8, u8)> {
    let answer = termbg::rgb(Duration::from_millis(120))
        .ok()
        .map(|rgb| ((rgb.r >> 8) as u8, (rgb.g >> 8) as u8, (rgb.b >> 8) as u8));
    if let Some(path) = term_bg_path() {
        let _ = std::fs::create_dir_all(path.parent().unwrap());
        let value = match answer {
            Some((r, g, b)) => format!("{:02x}{:02x}{:02x}", r, g, b),
            None => "none".to_string(),
        };
        let old = std::fs::read_to_string(&path).unwrap_or_default();
        let _ = std::fs::write(path, update_term_bg(&old, &terminal_identity(), &value));
    }
    answer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_term_bg_is_remembered_per_terminal() {
        // tmux couldn't answer; foot could. Each keeps its own answer.
        let text = update_term_bg("", "tmux+foot", "none");
        let text = update_term_bg(&text, "foot", "fafafa");
        assert!(matches!(parse_term_bg(&text, "tmux+foot"), Some(TermBg::Unanswered)));
        assert!(matches!(parse_term_bg(&text, "foot"), Some(TermBg::Color(0xfa, 0xfa, 0xfa))));
        assert!(parse_term_bg(&text, "alacritty").is_none(), "unknown terminals get asked");

        // A new answer replaces that terminal's line only
        let text = update_term_bg(&text, "foot", "1e2026");
        assert!(matches!(parse_term_bg(&text, "foot"), Some(TermBg::Color(0x1e, 0x20, 0x26))));
        assert!(matches!(parse_term_bg(&text, "tmux+foot"), Some(TermBg::Unanswered)));
        assert_eq!(text.lines().count(), 2);
    }

    #[test]
    fn test_old_single_value_term_bg_file_is_ignored() {
        // The previous format held one value for every terminal
        assert!(parse_term_bg("none\n", "foot").is_none());
        assert_eq!(update_term_bg("none\n", "foot", "fafafa"), "foot fafafa\n");
    }
}
