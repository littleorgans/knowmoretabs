//! The one place user-facing bytes reach the terminal.
//!
//! slice: capture
//! why: `println!` panics when the write fails, and the writes here fail
//!      routinely: `| head` closes the pipe, a disk fills, a terminal goes
//!      away. A process must never die reporting work it already finished,
//!      and `serve` must not die at all, so every command writes through
//!      these four functions and the macros are denied elsewhere.

use std::io::{self, Write};

/// One line to stdout.
pub fn line(text: &str) {
    stdout(&format!("{text}\n"));
}

/// Text that already carries its own line breaks, written as it stands.
pub fn block(text: &str) {
    stdout(text);
}

/// One JSON document and its newline. The document is rendered whole before
/// anything is written, so a reader that goes away can only truncate it;
/// this never emits a document assembled in pieces, and never emits a second
/// one after the first has failed.
pub fn json(value: &serde_json::Value) {
    stdout(&format!("{value}\n"));
}

/// One line to stderr, for warnings and for the error a command exits on.
pub fn problem(text: &str) {
    let mut err = io::stderr().lock();
    let _ = writeln!(err, "{text}");
    let _ = err.flush();
}

/// The dropped error is the point: the caller has already done the work and
/// has nowhere left to report that the report itself went nowhere. The exit
/// status stays the one the work earned.
fn stdout(text: &str) {
    let mut out = io::stdout().lock();
    let _ = out.write_all(text.as_bytes());
    let _ = out.flush();
}
