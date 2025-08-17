use std::io::Write;
use std::process::Command;
use std::process::Stdio;

use anyhow::Context;
use anyhow::Result;

fn main() -> Result<()> {
    // Kill any existing session
    let _ = Command::new("tmux")
        .args(&["kill-session", "-t", "djls-debug"])
        .output();

    let _ = Command::new("pkill").args(&["-f", "lsp-devtools"]).output();

    // Start tmux in control mode
    let mut tmux = Command::new("tmux")
        .args(&["-C", "-f", "/dev/null"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("Failed to start tmux control mode")?;

    let stdin = tmux.stdin.as_mut().context("Failed to get tmux stdin")?;

    // Create session with editor
    writeln!(
        stdin,
        "new-session -d -s djls-debug 'nvim tests/project/djls_app/templates/djls_app/base.html'"
    )?;

    // Add devtools pane (20% width on the right)
    writeln!(
        stdin,
        "split-window -t djls-debug -h -p 20 'just dev devtools record'"
    )?;

    // Split the right pane horizontally for server logs (50/50 split)
    writeln!(
        stdin,
        "split-window -t djls-debug:0.1 -v -p 50 'tail -f /tmp/djls.log'"
    )?;

    // Set pane titles
    writeln!(stdin, "select-pane -t djls-debug:0.0 -T 'Editor'")?;
    writeln!(stdin, "select-pane -t djls-debug:0.1 -T 'LSP DevTools'")?;
    writeln!(stdin, "select-pane -t djls-debug:0.2 -T 'Server Logs'")?;

    // Enable pane borders with titles at the top
    writeln!(stdin, "set -t djls-debug pane-border-status top")?;

    // Add custom keybind to kill session (capital K)
    writeln!(stdin, "bind-key K kill-session")?;

    // Configure status bar with keybind hints
    writeln!(stdin, "set -t djls-debug status on")?;
    writeln!(stdin, "set -t djls-debug status-position bottom")?;
    writeln!(
        stdin,
        "set -t djls-debug status-style 'bg=colour235,fg=colour250'"
    )?;

    // Left side: session name
    writeln!(stdin, "set -t djls-debug status-left '[#S] '")?;
    writeln!(stdin, "set -t djls-debug status-left-length 20")?;

    // Right side: keybind hints
    writeln!(stdin, "set -t djls-debug status-right ' C-b d: detach | C-b K: kill session | C-b x: kill pane | C-b z: zoom | C-b ?: help '")?;
    writeln!(stdin, "set -t djls-debug status-right-length 90")?;

    // Center: window name
    writeln!(stdin, "set -t djls-debug status-justify centre")?;

    // Focus editor pane
    writeln!(stdin, "select-pane -t djls-debug:0.0")?;

    // Exit control mode
    writeln!(stdin, "detach")?;
    stdin.flush()?;

    // Close stdin to signal we're done sending commands
    drop(tmux.stdin.take());

    // Wait for control mode to finish
    tmux.wait()?;

    // Attach to session
    Command::new("tmux")
        .args(&["attach-session", "-t", "djls-debug"])
        .status()
        .context("Failed to attach to session")?;

    // Cleanup on exit
    let _ = Command::new("pkill").args(&["-f", "lsp-devtools"]).output();

    Ok(())
}
