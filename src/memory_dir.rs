//! Point a session's auto-memory at the real `~/.claude/projects` store.
//!
//! A shared runtime's `projects` is a symlink to `~/.claude/projects`. Claude
//! Code derives its default auto-memory dir as
//! `$CLAUDE_CONFIG_DIR/projects/<key>/memory/`, i.e. THROUGH that link, and its
//! "auto memory files are allowed for writing" exemption then fails against the
//! resolved target: every memory write prompts ("outside working directory ...
//! via a symlink"), and no `Edit(...)` allow rule clears it. Naming the resolved
//! dir in `autoMemoryDirectory` restores the exemption.
//!
//! The key must match Claude Code's own, or the session silently writes to an
//! empty memory dir. So this is all-or-nothing: any input the port below does
//! not cover (a non-ASCII path, a caller-supplied `--settings`, a project dir
//! that does not exist yet) leaves the launch exactly as it was.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

/// Sanitized names longer than this get truncated plus a hash suffix.
const MAX_KEY_LEN: usize = 200;

/// Claude Code's project-dir override: honoured whenever `CLAUDE_CONFIG_DIR` is
/// set, which it always is under clauth.
const PROJECT_DIR_NAME_ENV: &str = "CLAUDE_CODE_PROJECT_DIR_NAME";

/// The auto-memory dir to pin for a session launched in `cwd` against
/// `config_dir`, or `None` when the default is already right or cannot be
/// derived with certainty.
pub(crate) fn auto_memory_dir(
    cwd: &Path,
    config_dir: &Path,
    dir_name_override: Option<&str>,
) -> Option<PathBuf> {
    let projects = config_dir.join("projects");
    // Isolated runtimes own a real `projects` dir; the exemption holds there.
    if !fs::symlink_metadata(&projects)
        .ok()?
        .file_type()
        .is_symlink()
    {
        return None;
    }
    let store = fs::canonicalize(&projects).ok()?;
    let key = match dir_name_override.and_then(valid_dir_name) {
        Some(name) => name.to_string(),
        None => project_key(canonical_root(&fs::canonicalize(cwd).ok()?).to_str()?)?,
    };
    let project = store.join(key);
    // Only a key Claude Code has already used is trusted: a mismatch then lands
    // on an absent dir and falls back, rather than starting an empty memory.
    project.is_dir().then(|| project.join("memory"))
}

/// Append `--settings {"autoMemoryDirectory": ...}` when [`auto_memory_dir`]
/// has an answer and the caller passed no `--settings` of its own (two would
/// leave which one wins up to Claude Code).
pub(crate) fn pin_auto_memory_dir<S: AsRef<OsStr>>(
    command: &mut std::process::Command,
    cwd: &Path,
    config_dir: &Path,
    caller_args: &[S],
) {
    let caller_settings = caller_args.iter().any(|a| {
        let a = a.as_ref().to_string_lossy();
        a == "--settings" || a.starts_with("--settings=")
    });
    if caller_settings {
        return;
    }
    let dir_name_override = child_env(command, PROJECT_DIR_NAME_ENV);
    let Some(dir) = auto_memory_dir(cwd, config_dir, dir_name_override.as_deref()) else {
        return;
    };
    let Some(dir) = dir.to_str() else {
        return;
    };
    let settings = serde_json::json!({ "autoMemoryDirectory": dir }).to_string();
    command.arg("--settings").arg(settings);
}

/// The value `key` will have in the child: an explicit set or removal on
/// `command` wins over this process's environment.
fn child_env(command: &std::process::Command, key: &str) -> Option<String> {
    match command.get_envs().find(|(k, _)| *k == OsStr::new(key)) {
        Some((_, value)) => value.and_then(|v| v.to_str()).map(str::to_string),
        None => std::env::var(key).ok(),
    }
}

/// `CLAUDE_CODE_PROJECT_DIR_NAME` as Claude Code accepts it: 1-64 of
/// `[A-Za-z0-9_-]`, never a reserved Windows device name.
fn valid_dir_name(name: &str) -> Option<&str> {
    let charset = !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    let lower = name.to_ascii_lowercase();
    let reserved = matches!(lower.as_str(), "con" | "prn" | "aux" | "nul")
        || ((lower.starts_with("com") || lower.starts_with("lpt"))
            && lower.len() == 4
            && lower.as_bytes()[3].is_ascii_digit());
    (charset && !reserved).then_some(name)
}

/// Claude Code's project key for `base`: every non-alphanumeric UTF-16 unit
/// becomes `-`, and past [`MAX_KEY_LEN`] the name is cut and suffixed with a
/// base-36 hash of `base`. ASCII only: Claude Code NFC-normalizes first, which
/// changes both the length and the hash of a non-ASCII path.
pub(crate) fn project_key(base: &str) -> Option<String> {
    if !base.is_ascii() {
        return None;
    }
    let sanitized: String = base
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    if sanitized.len() <= MAX_KEY_LEN {
        return Some(sanitized);
    }
    Some(format!(
        "{}-{}",
        &sanitized[..MAX_KEY_LEN],
        to_base36(i64::from(java_hash(base)).unsigned_abs())
    ))
}

/// `s.split('').reduce((h, c) => (h << 5) - h + c.charCodeAt(0) | 0, 0)`.
fn java_hash(s: &str) -> i32 {
    s.encode_utf16().fold(0i32, |h, c| {
        (h << 5).wrapping_sub(h).wrapping_add(i32::from(c))
    })
}

fn to_base36(mut n: u64) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".to_string();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(char::from(DIGITS[(n % 36) as usize]));
        n /= 36;
    }
    out.iter().rev().collect()
}

/// The dir Claude Code keys memory on: a linked worktree's main working copy,
/// else the enclosing git root, else `cwd` itself. `cwd` must be canonical.
fn canonical_root(cwd: &Path) -> PathBuf {
    match git_root(cwd) {
        Some(root) => main_worktree(&root).unwrap_or(root),
        None => cwd.to_path_buf(),
    }
}

/// The nearest ancestor (or `dir` itself) holding a `.git` dir or file.
fn git_root(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .find(|d| {
            fs::symlink_metadata(d.join(".git"))
                .is_ok_and(|m| m.file_type().is_dir() || m.file_type().is_file())
        })
        .map(Path::to_path_buf)
}

/// For a linked worktree at `root`, the main working copy (or the bare repo dir)
/// it belongs to. `None` for an ordinary checkout or anything that does not
/// check out the way git writes it, which Claude Code also keys on `root`.
fn main_worktree(root: &Path) -> Option<PathBuf> {
    let dot_git = fs::read_to_string(root.join(".git")).ok()?;
    let gitdir = root.join(dot_git.trim().strip_prefix("gitdir:")?.trim());
    let common = gitdir.join(fs::read_to_string(gitdir.join("commondir")).ok()?.trim());
    let common = fs::canonicalize(common).ok()?;
    if fs::canonicalize(gitdir.parent()?).ok()? != common.join("worktrees") {
        return None;
    }
    let back = gitdir.join(fs::read_to_string(gitdir.join("gitdir")).ok()?.trim());
    if fs::canonicalize(back).ok()? != root.join(".git") {
        return None;
    }
    if common.file_name()? == ".git" {
        return common.parent().map(Path::to_path_buf);
    }
    // A bare repo's worktree keys on the bare dir, unless that holds a `.git`.
    (!common.join(".git").exists()).then_some(common)
}

#[cfg(test)]
#[path = "../tests/inline/memory_dir.rs"]
mod tests;
