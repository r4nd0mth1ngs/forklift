//! Git-style paging. When human output goes to a terminal, pipe it through a pager so long
//! output (history, a big diff, …) is scrollable instead of scrolling off the top. This
//! matches git exactly:
//!
//! * A pager is used **only** when stdout is a terminal — never for `--json`, a pipe, or a
//!   file (those stream raw).
//! * The pager is `$FORKLIFT_PAGER`, then `$PAGER`, then `less`. An empty value (or `cat`)
//!   disables paging, the convention git uses for `core.pager=""`.
//! * For `less` with no `$LESS` set, we apply git's defaults `FRX`: **F** quit if the output
//!   fits one screen (so short output prints inline), **R** keep color, **X** don't clear.
//!
//! The implementation redirects the process's stdout file descriptor onto the pager's
//! stdin, so every existing `println!` flows through the pager with no per-command change.

/// A running pager the process's stdout is wired into. Dropping it is not enough — call
/// [`Pager::close`] once output is done to flush, signal EOF, and wait for the user to quit.
#[cfg(unix)]
pub struct Pager {
    child: std::process::Child,
}

/// Restore the default disposition of `SIGPIPE`. Rust ignores it by default, which turns a
/// write to a quit pager (or a closed `| head`) into a panic; the default disposition makes
/// the process exit quietly at that point instead — how git and other pager-driven CLIs
/// behave, and what lets a bounded `history` walk stop when the reader stops reading.
#[cfg(unix)]
pub fn restore_sigpipe() {
    // SAFETY: setting a signal to its default disposition is async-signal-safe and this runs
    // once at startup before any threads that could race it.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

/// Set up a pager for human output on a terminal, or return `None` to stream raw.
#[cfg(unix)]
pub fn setup(no_pager: bool) -> Option<Pager> {
    use std::io::IsTerminal;
    use std::os::unix::io::AsRawFd;
    use std::process::{Command, Stdio};

    // No pager for --json (one machine document), when disabled, or when stdout is not a
    // terminal — a pipe or file streams raw, exactly like git.
    if no_pager || crate::output::is_json() || !std::io::stdout().is_terminal() {
        return None;
    }

    let configured = std::env::var("FORKLIFT_PAGER")
        .or_else(|_| std::env::var("PAGER"))
        .unwrap_or_else(|_| "less".to_string());
    let configured = configured.trim();
    if configured.is_empty() || configured == "cat" {
        return None;
    }

    // Allow a pager string with arguments, e.g. "less -R".
    let mut words = configured.split_whitespace();
    let program = words.next()?;
    let mut builder = Command::new(program);
    builder.args(words).stdin(Stdio::piped());

    if program == "less" && std::env::var_os("LESS").is_none() {
        builder.env("LESS", "FRX");
    }

    let mut child = builder.spawn().ok()?;
    let pager_stdin = child.stdin.take()?;

    // Point our stdout at the pager's stdin: from here every write flows into the pager.
    // `pager_stdin` is dropped right after, so fd 1 is the only handle keeping the pipe's
    // write end open — closing it in `close` is what gives the pager EOF.
    // SAFETY: dup2 onto a valid fd; the source fd is owned by `pager_stdin` until dropped.
    unsafe {
        libc::dup2(pager_stdin.as_raw_fd(), libc::STDOUT_FILENO);
    }
    drop(pager_stdin);

    Some(Pager { child })
}

#[cfg(unix)]
impl Pager {
    /// Flush our output, signal end-of-input to the pager, and wait for the user to quit it
    /// (so the shell prompt does not return underneath a still-open pager).
    pub fn close(mut self) {
        use std::io::Write;
        let _ = std::io::stdout().flush();
        // SAFETY: closing fd 1 hands the pager EOF; nothing writes to stdout afterwards
        // (a human error goes to stderr, and --json never installs a pager).
        unsafe {
            libc::close(libc::STDOUT_FILENO);
        }
        let _ = self.child.wait();
    }
}

// No pager on non-Unix targets: output prints straight through.
#[cfg(not(unix))]
pub struct Pager;

#[cfg(not(unix))]
pub fn restore_sigpipe() {}

#[cfg(not(unix))]
pub fn setup(_no_pager: bool) -> Option<Pager> {
    None
}

#[cfg(not(unix))]
impl Pager {
    pub fn close(self) {}
}
