use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_rim"))
}

fn unique_temp(prefix: &str) -> PathBuf {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let p = std::env::temp_dir().join(format!("rim-test-{prefix}-{n}"));
    fs::create_dir_all(&p).expect("create temp dir");
    p
}

fn make_project() -> PathBuf {
    let project = unique_temp("project");
    fs::write(
        project.join("package.json"),
        "{\"scripts\":{\"dev\":\"node index.js\"}}\n",
    )
    .expect("package.json");
    project
}

#[test]
fn prepare_links_node_modules_to_ram_base_and_keeps_source_on_disk() {
    let project = make_project();
    let base = unique_temp("base");

    let out = Command::new(bin())
        .arg("prepare")
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("run rim prepare");

    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let link = project.join("node_modules");
    let meta = fs::symlink_metadata(&link).expect("node_modules metadata");
    assert!(
        meta.file_type().is_symlink(),
        "node_modules should be a symlink"
    );

    let target = fs::read_link(&link).expect("read node_modules link");
    assert!(
        target.starts_with(&base),
        "target {target:?} should live under {base:?}"
    );
    assert!(
        target.join(".rim-keep").exists(),
        "target directory should exist in RAM base"
    );
    assert!(
        project.join("package.json").exists(),
        "source files stay in project directory"
    );
}

#[test]
fn refuses_to_overwrite_real_node_modules_directory() {
    let project = make_project();
    let base = unique_temp("base");
    fs::create_dir(project.join("node_modules")).expect("real node_modules");

    let out = Command::new(bin())
        .arg("prepare")
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("run rim prepare");

    assert!(
        !out.status.success(),
        "prepare should fail when node_modules is a real directory"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("node_modules exists and is not a symlink"),
        "stderr: {stderr}"
    );
}

#[test]
fn dry_run_wrapper_reports_env_and_does_not_execute_tool() {
    let project = make_project();
    let base = unique_temp("base");

    let out = Command::new(bin())
        .args(["--dry-run", "npm", "install"])
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("run rim dry-run");

    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("command: npm install"), "stdout: {stdout}");
    assert!(stdout.contains("npm_config_cache="), "stdout: {stdout}");
    assert!(stdout.contains("XDG_CACHE_HOME="), "stdout: {stdout}");
    assert!(
        project.join("node_modules").is_symlink(),
        "dry-run still prepares symlink"
    );
}

#[test]
fn install_like_commands_run_in_ram_shadow_and_copy_lock_back() {
    let project = make_project();
    let base = unique_temp("base");
    let fake_bin = unique_temp("bin");
    let fake_npm = fake_bin.join("npm");
    fs::write(
        &fake_npm,
        "#!/usr/bin/env bash\nset -euo pipefail\npwd > npm-cwd.txt\nprintf '{\"lockfileVersion\":3}\n' > package-lock.json\nmkdir -p node_modules/fake-pkg\n",
    )
    .expect("fake npm");
    let chmod = Command::new("chmod")
        .arg("+x")
        .arg(&fake_npm)
        .status()
        .expect("chmod fake npm");
    assert!(chmod.success());

    let path = format!(
        "{}:{}",
        fake_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let out = Command::new(bin())
        .args(["npm", "install"])
        .env("RIM_BASE", &base)
        .env("PATH", path)
        .current_dir(&project)
        .output()
        .expect("run rim fake npm install");

    assert!(
        out.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let link = project.join("node_modules");
    assert!(
        link.is_symlink(),
        "project node_modules should remain a symlink"
    );
    let target = fs::read_link(&link).expect("read link");
    assert!(
        target.starts_with(&base),
        "target should be under RAM base: {target:?}"
    );
    assert!(
        target.join("fake-pkg").exists(),
        "fake package installed in RAM node_modules"
    );
    assert!(
        project.join("package-lock.json").exists(),
        "install-generated lockfile should be copied back to source project"
    );
}

#[test]
fn install_like_commands_copy_package_json_mutations_back() {
    let project = make_project();
    let base = unique_temp("base");
    let fake_bin = unique_temp("bin");
    let fake_npm = fake_bin.join("npm");
    fs::write(
        &fake_npm,
        "#!/usr/bin/env bash\nset -euo pipefail\nprintf '{\"dependencies\":{\"fake\":\"1.0.0\"}}\n' > package.json\nprintf '{\"lockfileVersion\":3}\n' > package-lock.json\nmkdir -p node_modules/fake\n",
    )
    .expect("fake npm");
    let chmod = Command::new("chmod")
        .arg("+x")
        .arg(&fake_npm)
        .status()
        .expect("chmod fake npm");
    assert!(chmod.success());

    let path = format!(
        "{}:{}",
        fake_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let out = Command::new(bin())
        .args(["npm", "install", "fake@1.0.0"])
        .env("RIM_BASE", &base)
        .env("PATH", path)
        .current_dir(&project)
        .output()
        .expect("run rim fake npm install package");

    assert!(
        out.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let package_json = fs::read_to_string(project.join("package.json")).expect("package.json");
    assert!(
        package_json.contains("fake"),
        "package.json mutations from install-like commands should be copied back"
    );
}

#[test]
fn prepare_shadow_removes_stale_manifests_absent_from_source() {
    let project = make_project();
    let base = unique_temp("base");

    let first = Command::new(bin())
        .arg("prepare")
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("first prepare");
    assert!(first.status.success());

    let link_target = fs::read_link(project.join("node_modules")).expect("read link");
    let shadow_project = link_target.parent().expect("node_modules parent");
    fs::write(shadow_project.join("package-lock.json"), "stale lock").expect("stale lock");

    let second = Command::new(bin())
        .arg("prepare")
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("second prepare");
    assert!(second.status.success());

    assert!(
        !shadow_project.join("package-lock.json").exists(),
        "shadow manifests absent from source should be removed before installs"
    );
}

#[test]
fn pnpm_wrapper_injects_store_dir_before_subcommand() {
    let project = make_project();
    let base = unique_temp("base");

    let out = Command::new(bin())
        .args(["--dry-run", "pnpm", "install"])
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("run rim dry-run pnpm");

    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("command: pnpm --store-dir"),
        "stdout: {stdout}"
    );
    assert!(stdout.contains("pnpm-store"), "stdout: {stdout}");
}

#[test]
fn clean_removes_only_current_projects_ram_directory_and_dead_symlink() {
    let project = make_project();
    let base = unique_temp("base");

    let prepare = Command::new(bin())
        .arg("prepare")
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("prepare");
    assert!(
        prepare.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&prepare.stderr)
    );

    let link = project.join("node_modules");
    let target = fs::read_link(&link).expect("read link");
    assert!(target.exists());

    let clean = Command::new(bin())
        .arg("clean")
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("clean");
    assert!(
        clean.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&clean.stderr)
    );

    assert!(
        !target.exists(),
        "RAM dependency directory should be removed"
    );
    assert!(
        !Path::new(&link).exists(),
        "node_modules symlink should be removed after clean"
    );
}
