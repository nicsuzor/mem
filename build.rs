fn main() {
    // Embed short git commit SHA (with -dirty suffix if uncommitted changes exist)
    let hash = std::env::var("BUILD_GIT_HASH").unwrap_or_else(|_| {
        std::process::Command::new("git")
            .args(["describe", "--always", "--dirty", "--abbrev=7", "--exclude=*"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "unknown".to_string())
    });
    println!("cargo:rustc-env=BUILD_GIT_HASH={}", hash.trim());

    // Rerun when commits change
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
}
