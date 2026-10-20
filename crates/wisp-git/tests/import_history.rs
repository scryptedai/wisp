use std::process::Command;
use wisp_engine::repo::{Repository, Unlock, MAIN_REF};
use wisp_git::{import_git, ImportGitOptions};
use wisp_store::Ledger;

#[test]
fn import_git_history_maps_commits() {
    let dir = tempfile::tempdir().unwrap();
    let git_dir = dir.path().join("src-git");
    std::fs::create_dir_all(&git_dir).unwrap();
    Command::new("git")
        .args(["init"])
        .current_dir(&git_dir)
        .output()
        .unwrap();
    std::fs::write(
        git_dir.join("a.md"),
        "---\ntype: Note\ntitle: A\n---\n\n## One\n\nv1\n",
    )
    .unwrap();
    Command::new("git")
        .args(["add", "a.md"])
        .current_dir(&git_dir)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "first"])
        .current_dir(&git_dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    std::fs::write(
        git_dir.join("a.md"),
        "---\ntype: Note\ntitle: A\n---\n\n## One\n\nv2\n",
    )
    .unwrap();
    Command::new("git")
        .args(["commit", "-am", "second"])
        .current_dir(&git_dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();

    let bundle = dir.path().join("bundle");
    std::fs::create_dir_all(&bundle).unwrap();
    let mut repo = Repository::init(&bundle, "0.2", &Unlock::default()).unwrap();
    let v = import_git(
        &mut repo,
        &git_dir,
        ImportGitOptions {
            prefix: None,
            git_ref: None,
            squash_message: None,
            history: true,
        },
    )
    .unwrap();
    assert_eq!(v["mode"], "history");
    assert!(v["wisp_changesets"].as_u64().unwrap() >= 1);
    assert!(repo.wisp_dir.join("meta/git-map.json").is_file());
    let head = Ledger::head(repo.ledger.conn(), MAIN_REF).unwrap();
    assert!(head.is_some());
}
