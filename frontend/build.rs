use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(repo: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let value = String::from_utf8(output.stdout).ok()?;
    Some(value.trim().to_owned())
}

fn track_git_path(repo: &Path, path: &str) {
    if let Some(path) = git(repo, &["rev-parse", "--git-path", path]) {
        let path = PathBuf::from(path);
        let path = if path.is_absolute() {
            path
        } else {
            repo.join(path)
        };
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let repo = manifest_dir.parent().unwrap_or(&manifest_dir);

    // HEAD changes for detached checkouts; the branch ref changes for normal development.
    track_git_path(repo, "HEAD");
    if let Some(head_ref) = git(repo, &["symbolic-ref", "-q", "HEAD"]) {
        track_git_path(repo, &head_ref);
    }
    track_git_path(repo, "packed-refs");
    track_git_path(repo, "shallow");

    let version =
        if git(repo, &["rev-parse", "--is-shallow-repository"]).as_deref() == Some("false") {
            git(repo, &["rev-list", "--count", "HEAD"])
                .zip(git(repo, &["rev-parse", "--short", "HEAD"]))
                .map(|(count, hash)| format!("r{count}-g{hash}"))
                .unwrap_or_else(|| "unknown".to_owned())
        } else {
            "unknown".to_owned()
        };

    println!("cargo:rustc-env=WASM_IDE_VERSION={version}");
}
