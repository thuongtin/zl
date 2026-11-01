//! Terminal I/O: credential prompts + a repainted chat list.
//!
//! Deliberately dependency-free (no ratatui): ANSI + Win32 console mode is all it needs.

use std::io::{self, BufRead, Write};

use anyhow::Result;

use crate::chat::Thread;

pub enum Action {
    None,
    Up,
    Down,
    Refresh,
    Quit,
}

pub fn prompt(label: &str) -> Result<String> {
    print!("{label}");
    io::stdout().flush()?;
    let mut s = String::new();
    io::stdin().lock().read_line(&mut s)?;
    Ok(s.trim_end_matches(['\r', '\n']).to_string())
}

pub fn prompt_default(label: &str, default: &str) -> Result<String> {
    let v = prompt(&format!("{label} [{default}]: "))?;
    Ok(if v.trim().is_empty() {
        default.to_string()
    } else {
        v
    })
}

/// Password prompt.
///
/// Deliberately does NOT touch console mode. Toggling `ENABLE_ECHO_INPUT` around
/// `std::io::stdin()` deadlocks on Windows because the Rust stdin handle keeps its own
/// line buffer and the line never completes while echo is off. Reading normally is
/// slightly worse UX but always works; `--pass-stdin` covers unattended runs.
pub fn prompt_password(label: &str) -> Result<String> {
    print!("{label}");
    io::stdout().flush()?;
    let mut s = String::new();
    io::stdin().lock().read_line(&mut s)?;
    Ok(s.trim_end_matches(['\r', '\n']).to_string())
}

/// Return the current time in the operating system timezone.
pub fn clock() -> String {
    crate::clock::now_clock()
}

/// Repaint the thread list and wait for one key.
pub fn draw(threads: &[Thread], selected: usize, status: &str) -> Result<Action> {
    let mut out = io::stdout();
    write!(out, "\x1b[H\x1b[J")?;

    writeln!(
        out,
        "\x1b[1mZalo\x1b[0m  {} hội thoại  ·  {status}",
        threads.len()
    )?;
    writeln!(out, "{}", "─".repeat(64))?;

    if threads.is_empty() {
        writeln!(out, "  (chưa có hội thoại)")?;
    }

    for (i, t) in threads.iter().take(25).enumerate() {
        let marker = if i == selected { "▸" } else { " " };
        let name = fit(&t.name, 24);
        let prev = fit(&t.preview, 32);
        if i == selected {
            writeln!(out, "\x1b[7m{marker} {name}  {prev}\x1b[0m")?;
        } else {
            writeln!(out, "{marker} {name}  \x1b[90m{prev}\x1b[0m")?;
        }
    }

    writeln!(out, "{}", "─".repeat(64))?;
    writeln!(
        out,
        "\x1b[90m↑/↓ chọn (mỗi dòng một phím) · r làm mới · q thoát\x1b[0m"
    )?;
    out.flush()?;

    // Line-buffered read: no terminal mode juggling, so it can never hang the console.
    let mut line = String::new();
    if io::stdin().lock().read_line(&mut line).is_err() {
        return Ok(Action::None);
    }
    let cmd = line.trim();

    Ok(match cmd {
        "q" | "Q" => Action::Quit,
        "r" | "R" => Action::Refresh,
        "k" => Action::Up,
        "j" => Action::Down,
        _ => Action::None,
    })
}

fn fit(s: &str, w: usize) -> String {
    let mut out = String::with_capacity(w);
    for ch in s.chars() {
        if out.chars().count() >= w {
            break;
        }
        out.push(if ch == '\n' || ch == '\t' { ' ' } else { ch });
    }
    let pad = w.saturating_sub(out.chars().count());
    out.extend(std::iter::repeat_n(' ', pad));
    out
}
