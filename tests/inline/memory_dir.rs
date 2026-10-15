use super::*;
use std::fs;

/// A canonical tempdir: macOS's `/var` is a symlink to `/private/var`, and the
/// code under test keys on canonical paths.
fn tmp() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = fs::canonicalize(dir.path()).expect("canonicalize tempdir");
    (dir, path)
}

/// A shared-runtime layout: `<root>/store` is the real projects store and
/// `<root>/runtime/projects` links to it.
#[cfg(unix)]
fn shared_runtime(root: &Path) -> (PathBuf, PathBuf) {
    let store = root.join("store");
    let runtime = root.join("runtime");
    fs::create_dir_all(&store).expect("create store");
    fs::create_dir_all(&runtime).expect("create runtime");
    std::os::unix::fs::symlink(&store, runtime.join("projects")).expect("link projects");
    (store, runtime)
}

/// What `git worktree add <linked>` writes for a repo checked out at `main`.
fn link_worktree(main: &Path, linked: &Path, name: &str) {
    let admin = main.join(".git").join("worktrees").join(name);
    fs::create_dir_all(&admin).expect("create worktree admin dir");
    fs::create_dir_all(linked).expect("create linked worktree");
    fs::write(
        linked.join(".git"),
        format!("gitdir: {}\n", admin.display()),
    )
    .expect("write .git");
    fs::write(admin.join("commondir"), "../..\n").expect("write commondir");
    fs::write(
        admin.join("gitdir"),
        format!("{}\n", linked.join(".git").display()),
    )
    .expect("write gitdir");
}

#[test]
fn project_key_replaces_every_non_alphanumeric() {
    assert_eq!(
        project_key("/Users/cliph/.homesick/repos/dotfiles").as_deref(),
        Some("-Users-cliph--homesick-repos-dotfiles"),
    );
}

#[test]
fn project_key_hashes_a_long_path_like_claude_code() {
    // Expected value computed by Claude Code's own key function (2.1.300).
    let long = format!("/Users/someone/{}project", "deeply/nested.dir_".repeat(12));
    let want = "-Users-someone-deeply-nested-dir-deeply-nested-dir-deeply-nested-dir-\
                deeply-nested-dir-deeply-nested-dir-deeply-nested-dir-deeply-nested-dir-\
                deeply-nested-dir-deeply-nested-dir-deeply-nested-dir-deepl-g97z6";
    assert_eq!(project_key(&long).as_deref(), Some(want));
}

#[test]
fn project_key_refuses_non_ascii() {
    assert_eq!(project_key("/Users/zoë/repo"), None);
}

#[test]
fn java_hash_wraps_like_int32() {
    assert_eq!(java_hash(""), 0);
    assert_eq!(java_hash("a"), 97);
    // Long enough to overflow i32 many times over.
    assert_eq!(java_hash(&"z".repeat(64)), {
        let mut h = 0i32;
        for _ in 0..64 {
            h = h.wrapping_mul(31).wrapping_add(122);
        }
        h
    });
}

#[test]
fn canonical_root_walks_up_to_the_git_root() {
    let (_guard, root) = tmp();
    let repo = root.join("repo");
    let sub = repo.join("src").join("deep");
    fs::create_dir_all(repo.join(".git")).expect("create .git");
    fs::create_dir_all(&sub).expect("create subdir");
    assert_eq!(canonical_root(&sub), repo);
}

#[test]
fn canonical_root_is_cwd_outside_git() {
    let (_guard, root) = tmp();
    let plain = root.join("plain");
    fs::create_dir_all(&plain).expect("create dir");
    assert_eq!(canonical_root(&plain), plain);
}

#[test]
fn canonical_root_maps_a_linked_worktree_to_its_main_checkout() {
    let (_guard, root) = tmp();
    let main = root.join("main");
    fs::create_dir_all(main.join(".git")).expect("create .git");
    let linked = root.join("elsewhere").join("feature");
    link_worktree(&main, &linked, "feature");
    assert_eq!(canonical_root(&linked), main);
    fs::create_dir_all(linked.join("src")).expect("create src");
    assert_eq!(canonical_root(&linked.join("src")), main);
}

#[test]
fn canonical_root_keeps_a_worktree_whose_back_link_disagrees() {
    let (_guard, root) = tmp();
    let main = root.join("main");
    fs::create_dir_all(main.join(".git")).expect("create .git");
    let linked = root.join("feature");
    link_worktree(&main, &linked, "feature");
    fs::write(
        main.join(".git/worktrees/feature/gitdir"),
        "/somewhere/else/.git\n",
    )
    .expect("rewrite gitdir");
    assert_eq!(canonical_root(&linked), linked);
}

#[cfg(unix)]
#[test]
fn auto_memory_dir_resolves_through_the_projects_link() {
    let (_guard, root) = tmp();
    let (store, runtime) = shared_runtime(&root);
    let repo = root.join("repo");
    fs::create_dir_all(repo.join(".git")).expect("create .git");
    let key = project_key(repo.to_str().expect("utf-8")).expect("ascii");
    fs::create_dir_all(store.join(&key)).expect("create project dir");

    assert_eq!(
        auto_memory_dir(&repo, &runtime, None),
        Some(store.join(key).join("memory")),
    );
}

#[cfg(unix)]
#[test]
fn auto_memory_dir_skips_a_project_claude_code_has_not_keyed_yet() {
    let (_guard, root) = tmp();
    let (_store, runtime) = shared_runtime(&root);
    let repo = root.join("repo");
    fs::create_dir_all(&repo).expect("create repo");
    assert_eq!(auto_memory_dir(&repo, &runtime, None), None);
}

#[test]
fn auto_memory_dir_skips_an_isolated_runtime() {
    let (_guard, root) = tmp();
    let runtime = root.join("runtime");
    let repo = root.join("repo");
    fs::create_dir_all(&repo).expect("create repo");
    let key = project_key(repo.to_str().expect("utf-8")).expect("ascii");
    fs::create_dir_all(runtime.join("projects").join(key)).expect("create real projects");
    assert_eq!(auto_memory_dir(&repo, &runtime, None), None);
}

#[cfg(unix)]
#[test]
fn auto_memory_dir_honours_a_valid_project_dir_name() {
    let (_guard, root) = tmp();
    let (store, runtime) = shared_runtime(&root);
    let repo = root.join("repo");
    fs::create_dir_all(&repo).expect("create repo");
    fs::create_dir_all(store.join("custom_name")).expect("create named dir");

    assert_eq!(
        auto_memory_dir(&repo, &runtime, Some("custom_name")),
        Some(store.join("custom_name").join("memory")),
    );
    // Claude Code ignores an invalid name and keys on the path; so do we.
    assert_eq!(auto_memory_dir(&repo, &runtime, Some("bad/name")), None);
    assert_eq!(auto_memory_dir(&repo, &runtime, Some("nul")), None);
}

#[cfg(unix)]
#[test]
fn pin_auto_memory_dir_appends_settings_or_nothing() {
    let (_guard, root) = tmp();
    let (store, runtime) = shared_runtime(&root);
    let repo = root.join("repo");
    fs::create_dir_all(&repo).expect("create repo");
    let key = project_key(repo.to_str().expect("utf-8")).expect("ascii");
    fs::create_dir_all(store.join(&key)).expect("create project dir");
    let args = |c: &std::process::Command| -> Vec<String> {
        c.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    };

    let mut pinned = std::process::Command::new("claude");
    pinned.env_remove(PROJECT_DIR_NAME_ENV);
    pin_auto_memory_dir(&mut pinned, &repo, &runtime, &[] as &[&str]);
    let want = serde_json::json!({
        "autoMemoryDirectory": store.join(&key).join("memory").to_str().expect("utf-8"),
    })
    .to_string();
    assert_eq!(args(&pinned), vec!["--settings".to_string(), want]);

    for caller in [
        ["--settings", "{}"].as_slice(),
        ["--settings={}"].as_slice(),
    ] {
        let mut left = std::process::Command::new("claude");
        left.env_remove(PROJECT_DIR_NAME_ENV);
        pin_auto_memory_dir(&mut left, &repo, &runtime, caller);
        assert!(
            left.get_args().next().is_none(),
            "a caller's own --settings must not be joined by a second one"
        );
    }
}
