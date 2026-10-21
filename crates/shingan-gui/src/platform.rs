//! Small OS integration helpers.

use std::path::Path;
use std::process::{Command, Stdio};

fn spawn(cmd: &mut Command) {
    let _ = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

/// Open a file or folder with the system's default application.
pub fn open_path(path: &Path) {
    #[cfg(target_os = "windows")]
    spawn(Command::new("explorer").arg(path));
    #[cfg(target_os = "macos")]
    spawn(Command::new("open").arg(path));
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    spawn(Command::new("xdg-open").arg(path));
}

/// Show a file in the system file manager (selected, where supported).
pub fn reveal_in_folder(path: &Path) {
    #[cfg(target_os = "windows")]
    {
        let mut arg = std::ffi::OsString::from("/select,");
        arg.push(path);
        spawn(Command::new("explorer").arg(arg));
    }
    #[cfg(target_os = "macos")]
    spawn(Command::new("open").arg("-R").arg(path));
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    if let Some(parent) = path.parent() {
        spawn(Command::new("xdg-open").arg(parent));
    }
}

/// Shorten to at most `max` characters (not bytes), keeping the start.
pub fn truncate_end(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let keep: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{keep}…")
}

/// Shorten to at most `max` characters, keeping the end (useful for paths).
pub fn truncate_start(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    let keep: String = s.chars().skip(n - max.saturating_sub(1)).collect();
    format!("…{keep}")
}

/// Cut a string to at most `max_bytes` bytes on a character boundary.
pub fn truncate_bytes(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Human-readable size.
pub fn format_size(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.2} GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_is_utf8_safe() {
        let name = "心眼で見つけた重複ファイルの名前.jpg";
        let t = truncate_end(name, 10);
        assert_eq!(t.chars().count(), 10);
        assert!(t.ends_with('…'));
        let t = truncate_start(name, 8);
        assert!(t.ends_with(".jpg"));
        assert_eq!(truncate_bytes("héllo", 2), "h");
        assert_eq!(truncate_bytes("abc", 10), "abc");
    }

    #[test]
    fn short_strings_are_unchanged() {
        assert_eq!(truncate_end("abc", 10), "abc");
        assert_eq!(truncate_start("abc", 10), "abc");
    }
}
