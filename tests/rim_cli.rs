use std::fs;
use std::os::unix::fs::symlink;
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
fn help_lists_cleanup_options() {
    let out = Command::new(bin())
        .arg("--help")
        .output()
        .expect("run rim --help");

    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    for expected in [
        "rim ls",
        "rim gc",
        "rim path",
        "rim explain",
        "rim scan",
        "rim adopt",
        "rim backup",
        "rim repair",
        "rim ensure",
        "rim pin|unpin",
        "rim manager",
        "--include-pinned",
        "--force",
        "install|run|test|start",
        "RIM_PROFILE",
        "--tmp",
        "--suggest",
        "--cache-only",
        "--deps-only",
        "--dry-run",
        "--auto-clean",
        "--ephemeral",
        "--keep-on-error",
        "--keep-cache",
    ] {
        assert!(stdout.contains(expected), "stdout: {stdout}");
    }
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
fn prepare_writes_metadata_and_ls_reports_layer() {
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

    let link_target = fs::read_link(project.join("node_modules")).expect("read link");
    let rim_dir = link_target
        .parent()
        .and_then(Path::parent)
        .expect("rim dir");
    let meta = fs::read_to_string(rim_dir.join(".rim-meta.json")).expect("rim metadata");
    assert!(meta.contains("\"project_root\""), "meta: {meta}");
    assert!(meta.contains("\"manager\": \"prepare\""), "meta: {meta}");
    assert!(meta.contains("\"manifest_hash\""), "meta: {meta}");
    assert!(meta.contains("\"pinned\": false"), "meta: {meta}");
    assert!(
        meta.contains(&project.to_string_lossy().to_string()),
        "meta should contain project path: {meta}"
    );

    let out = Command::new(bin())
        .arg("ls")
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("rim ls");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("PROJECT"), "stdout: {stdout}");
    assert!(stdout.contains("prepare"), "stdout: {stdout}");
    assert!(
        stdout.contains(&rim_dir.display().to_string()),
        "stdout: {stdout}"
    );
}

#[test]
fn path_command_prints_selected_paths() {
    let project = make_project();
    let base = unique_temp("base");

    let out = Command::new(bin())
        .arg("path")
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("rim path");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.trim().starts_with(&base.display().to_string()),
        "stdout: {stdout}"
    );

    let out = Command::new(bin())
        .args(["path", "--node-modules"])
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("rim path node_modules");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.trim().ends_with("project/node_modules"),
        "stdout: {stdout}"
    );
}

#[test]
fn explain_reports_install_plan_and_cache_trim() {
    let project = make_project();
    let base = unique_temp("base");

    let out = Command::new(bin())
        .args(["explain", "bun", "install"])
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("rim explain");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("install_like: true"), "stdout: {stdout}");
    assert!(stdout.contains("sync manifests"), "stdout: {stdout}");
    assert!(stdout.contains("trim bun cache"), "stdout: {stdout}");
}

#[test]
fn doctor_suggest_reports_heavy_packages_and_scripts() {
    let project = unique_temp("suggest-project");
    let base = unique_temp("suggest-base");
    fs::write(
        project.join("package.json"),
        "{\"dependencies\":{\"next\":\"16.0.0\"},\"scripts\":{\"postinstall\":\"prisma generate\"},\"workspaces\":[\"packages/*\"]}\n",
    )
    .expect("package.json");

    let out = Command::new(bin())
        .args(["doctor", "--suggest"])
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("rim doctor suggest");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("suggestions:"), "stdout: {stdout}");
    assert!(
        stdout.contains("heavy packages detected: next"),
        "stdout: {stdout}"
    );
    assert!(stdout.contains("workspace detected"), "stdout: {stdout}");
    assert!(
        stdout.contains("lifecycle scripts detected"),
        "stdout: {stdout}"
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
        stderr.contains("node_modules exists and is not a symlink. Try:"),
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
fn rim_profile_cache_uses_home_cache_when_base_is_unset() {
    let project = make_project();

    let out = Command::new(bin())
        .args(["path"])
        .env_remove("RIM_BASE")
        .env("RIM_PROFILE", "cache")
        .current_dir(&project)
        .output()
        .expect("rim path with cache profile");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("/.cache/rim/"), "stdout: {stdout}");
}

#[test]
fn auto_detects_bun_for_shortcut_commands() {
    let project = make_project();
    let base = unique_temp("base");
    fs::write(project.join("bun.lock"), "").expect("bun lock");

    let out = Command::new(bin())
        .args(["--dry-run", "install"])
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("rim shortcut install");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("command: bun install"), "stdout: {stdout}");
}

#[test]
fn auto_detects_npm_from_package_lock() {
    let project = make_project();
    let base = unique_temp("base");
    fs::write(
        project.join("package-lock.json"),
        "{\"lockfileVersion\":3}\n",
    )
    .expect("lock");

    let out = Command::new(bin())
        .args(["--dry-run", "run", "dev"])
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("rim shortcut run dev");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("command: npm run dev"), "stdout: {stdout}");
}

#[test]
fn deno_commands_do_not_create_project_node_modules_symlink() {
    let project = unique_temp("deno-project");
    let base = unique_temp("base");
    fs::write(project.join("deno.json"), "{}\n").expect("deno json");

    let out = Command::new(bin())
        .args(["--dry-run", "deno", "cache", "main.ts"])
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("rim deno dry-run");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !project.join("node_modules").exists(),
        "deno cache should not create a project node_modules symlink"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("command: deno cache main.ts"),
        "stdout: {stdout}"
    );
}

#[test]
fn ensure_installs_when_dependencies_are_missing() {
    let project = make_project();
    let base = unique_temp("base");
    fs::write(project.join("bun.lock"), "").expect("bun lock");
    let fake_bin = unique_temp("bin");
    let fake_bun = fake_bin.join("bun");
    fs::write(
        &fake_bun,
        "#!/usr/bin/env bash\nset -euo pipefail\nprintf '%s\n' \"$@\" >> \"$RIM_TEST_LOG\"\nmkdir -p node_modules/fake\nprintf lock > bun.lock\n",
    )
    .expect("fake bun");
    assert!(
        Command::new("chmod")
            .arg("+x")
            .arg(&fake_bun)
            .status()
            .unwrap()
            .success()
    );
    let log = project.join("calls.log");
    let path = format!(
        "{}:{}",
        fake_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let out = Command::new(bin())
        .arg("ensure")
        .env("RIM_BASE", &base)
        .env("PATH", path)
        .env("RIM_TEST_LOG", &log)
        .current_dir(&project)
        .output()
        .expect("rim ensure");
    assert!(
        out.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("dependencies missing"), "stdout: {stdout}");
    assert!(fs::read_to_string(&log).expect("log").contains("install"));
    let link_target = fs::read_link(project.join("node_modules")).expect("read link");
    assert!(
        link_target.join("fake").exists(),
        "ensure should install deps"
    );
}

#[test]
fn ensure_skips_when_dependencies_exist() {
    let project = make_project();
    let base = unique_temp("base");
    fs::write(project.join("bun.lock"), "").expect("bun lock");

    let prepare = Command::new(bin())
        .arg("prepare")
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("prepare");
    assert!(prepare.status.success());
    let link_target = fs::read_link(project.join("node_modules")).expect("read link");
    fs::create_dir_all(link_target.join("already-installed")).expect("dep");

    let out = Command::new(bin())
        .arg("ensure")
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("rim ensure");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("already present"), "stdout: {stdout}");
}

#[test]
fn shortcut_run_auto_installs_missing_dependencies_before_run() {
    let project = make_project();
    let base = unique_temp("base");
    fs::write(project.join("bun.lock"), "").expect("bun lock");
    let fake_bin = unique_temp("bin");
    let fake_bun = fake_bin.join("bun");
    fs::write(
        &fake_bun,
        "#!/usr/bin/env bash\nset -euo pipefail\nprintf '%s\n' \"$@\" >> \"$RIM_TEST_LOG\"\nif [ \"${1:-}\" = install ]; then mkdir -p node_modules/fake; printf lock > bun.lock; exit 0; fi\nif [ \"${1:-}\" = run ]; then test -d node_modules/fake; exit 7; fi\n",
    )
    .expect("fake bun");
    assert!(
        Command::new("chmod")
            .arg("+x")
            .arg(&fake_bun)
            .status()
            .unwrap()
            .success()
    );
    let log = project.join("calls.log");
    let path = format!(
        "{}:{}",
        fake_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let out = Command::new(bin())
        .args(["run", "dev"])
        .env("RIM_BASE", &base)
        .env("PATH", path)
        .env("RIM_TEST_LOG", &log)
        .current_dir(&project)
        .output()
        .expect("rim shortcut run dev");
    assert_eq!(out.status.code(), Some(7));
    let log = fs::read_to_string(log).expect("log");
    assert!(log.contains("install"), "log: {log}");
    assert!(log.contains("run\ndev"), "log: {log}");
}

#[test]
fn shortcut_run_dry_run_reports_ensure_install() {
    let project = make_project();
    let base = unique_temp("base");
    fs::write(project.join("bun.lock"), "").expect("bun lock");

    let out = Command::new(bin())
        .args(["--dry-run", "run", "dev"])
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("rim shortcut run dry-run");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("ensure_install: bun install"),
        "stdout: {stdout}"
    );
    assert!(stdout.contains("command: bun run dev"), "stdout: {stdout}");
}

#[test]
fn explain_auto_detects_shortcut_command() {
    let project = make_project();
    let base = unique_temp("base");
    fs::write(project.join("bun.lock"), "").expect("bun lock");

    let out = Command::new(bin())
        .args(["explain", "install"])
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("rim explain install");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("tool: bun"), "stdout: {stdout}");
    assert!(stdout.contains("args: install"), "stdout: {stdout}");
}

#[test]
fn manager_command_prefers_package_manager_field_over_lockfiles() {
    let project = make_project();
    let base = unique_temp("base");
    fs::write(
        project.join("package.json"),
        "{\"packageManager\":\"npm@10.9.4\",\"scripts\":{\"dev\":\"node index.js\"}}\n",
    )
    .expect("package json");
    fs::write(project.join("bun.lock"), "").expect("bun lock");

    let out = Command::new(bin())
        .arg("manager")
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("rim manager");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("detected: npm"), "stdout: {stdout}");
    assert!(stdout.contains("packageManager npm"), "stdout: {stdout}");
}

#[test]
fn ensure_reinstalls_when_metadata_is_missing_even_if_deps_exist() {
    let project = make_project();
    let base = unique_temp("base");
    fs::write(project.join("bun.lock"), "").expect("bun lock");

    let prepare = Command::new(bin())
        .arg("prepare")
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("prepare");
    assert!(prepare.status.success());
    let link_target = fs::read_link(project.join("node_modules")).expect("read link");
    fs::create_dir_all(link_target.join("existing-dep")).expect("dep");
    let rim_dir = link_target
        .parent()
        .and_then(Path::parent)
        .expect("rim dir");
    fs::remove_file(rim_dir.join(".rim-meta.json")).expect("remove meta");

    let fake_bin = unique_temp("bin");
    let fake_bun = fake_bin.join("bun");
    fs::write(
        &fake_bun,
        "#!/usr/bin/env bash\nset -euo pipefail\nprintf install >> \"$RIM_TEST_LOG\"\nmkdir -p node_modules/fake\nprintf lock > bun.lock\n",
    )
    .expect("fake bun");
    assert!(
        Command::new("chmod")
            .arg("+x")
            .arg(&fake_bun)
            .status()
            .unwrap()
            .success()
    );
    let log = project.join("calls.log");
    let path = format!(
        "{}:{}",
        fake_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let out = Command::new(bin())
        .arg("ensure")
        .env("RIM_BASE", &base)
        .env("PATH", path)
        .env("RIM_TEST_LOG", &log)
        .current_dir(&project)
        .output()
        .expect("ensure");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("manifest metadata missing"),
        "stdout: {stdout}"
    );
    assert!(fs::read_to_string(log).expect("log").contains("install"));
}

#[test]
fn ensure_reinstalls_when_manifest_hash_changes() {
    let project = make_project();
    let base = unique_temp("base");
    fs::write(project.join("bun.lock"), "").expect("bun lock");
    let fake_bin = unique_temp("bin");
    let fake_bun = fake_bin.join("bun");
    fs::write(
        &fake_bun,
        "#!/usr/bin/env bash\nset -euo pipefail\nprintf 'install\n' >> \"$RIM_TEST_LOG\"\nmkdir -p node_modules/fake\nprintf lock > bun.lock\n",
    )
    .expect("fake bun");
    assert!(
        Command::new("chmod")
            .arg("+x")
            .arg(&fake_bun)
            .status()
            .unwrap()
            .success()
    );
    let log = project.join("calls.log");
    let path = format!(
        "{}:{}",
        fake_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let first = Command::new(bin())
        .arg("ensure")
        .env("RIM_BASE", &base)
        .env("PATH", &path)
        .env("RIM_TEST_LOG", &log)
        .current_dir(&project)
        .output()
        .expect("first ensure");
    assert!(first.status.success());

    fs::write(
        project.join("package.json"),
        "{\"scripts\":{\"dev\":\"node index.js\"},\"dependencies\":{\"zod\":\"3.25.76\"}}\n",
    )
    .expect("mutate package json");

    let second = Command::new(bin())
        .arg("ensure")
        .env("RIM_BASE", &base)
        .env("PATH", &path)
        .env("RIM_TEST_LOG", &log)
        .current_dir(&project)
        .output()
        .expect("second ensure");
    assert!(second.status.success());
    let stdout = String::from_utf8_lossy(&second.stdout);
    assert!(stdout.contains("manifest hash changed"), "stdout: {stdout}");
    let calls = fs::read_to_string(log).expect("log");
    assert_eq!(calls.matches("install").count(), 2, "calls: {calls}");
}

#[test]
fn pin_protects_layer_from_gc_until_include_pinned() {
    let project = make_project();
    let driver = make_project();
    let base = unique_temp("base");

    let prepare = Command::new(bin())
        .arg("prepare")
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("prepare");
    assert!(prepare.status.success());
    let link_target = fs::read_link(project.join("node_modules")).expect("read link");
    let rim_dir = link_target
        .parent()
        .and_then(Path::parent)
        .expect("rim dir")
        .to_path_buf();

    let pin = Command::new(bin())
        .arg("pin")
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("pin");
    assert!(pin.status.success());
    let meta = fs::read_to_string(rim_dir.join(".rim-meta.json")).expect("meta");
    assert!(meta.contains("\"pinned\": true"), "meta: {meta}");

    let gc = Command::new(bin())
        .args(["gc", "--all"])
        .env("RIM_BASE", &base)
        .current_dir(&driver)
        .output()
        .expect("gc all");
    assert!(gc.status.success());
    assert!(rim_dir.exists(), "pinned layer should survive gc --all");

    let gc = Command::new(bin())
        .args(["gc", "--all", "--include-pinned"])
        .env("RIM_BASE", &base)
        .current_dir(&driver)
        .output()
        .expect("gc include pinned");
    assert!(gc.status.success());
    assert!(
        !rim_dir.exists(),
        "--include-pinned should allow deleting pinned layer"
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
fn bunfig_toml_is_synced_to_shadow_project() {
    let project = make_project();
    let base = unique_temp("base");
    fs::write(project.join("bunfig.toml"), "[install]\nexact = true\n").expect("bunfig");

    let out = Command::new(bin())
        .arg("prepare")
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("prepare");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let link_target = fs::read_link(project.join("node_modules")).expect("read link");
    let shadow_project = link_target.parent().expect("shadow project");
    assert!(
        shadow_project.join("bunfig.toml").exists(),
        "bunfig.toml should be copied to the shadow project"
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
fn doctor_reports_storage_and_project_risk_signals() {
    let project = unique_temp("doctor-project");
    let base = unique_temp("doctor-base");
    fs::write(
        project.join("package.json"),
        "{\"scripts\":{\"postinstall\":\"node setup.js\"},\"workspaces\":[\"packages/*\"]}\n",
    )
    .expect("package.json");

    let out = Command::new(bin())
        .arg("doctor")
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("run rim doctor");

    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("rim_base:"), "stdout: {stdout}");
    assert!(stdout.contains("mode:"), "stdout: {stdout}");
    assert!(stdout.contains("storage:"), "stdout: {stdout}");
    assert!(stdout.contains("memory:"), "stdout: {stdout}");
    assert!(stdout.contains("install_risk:"), "stdout: {stdout}");
    assert!(stdout.contains("workspace: detected"), "stdout: {stdout}");
    assert!(
        stdout.contains("lifecycle_scripts: detected"),
        "stdout: {stdout}"
    );
}

#[test]
fn status_counts_symlinks_without_following_targets() {
    let project = make_project();
    let base = unique_temp("base");
    let outside = unique_temp("outside");
    let outside_file = outside.join("big-file.bin");
    fs::write(&outside_file, vec![0_u8; 1024 * 1024]).expect("outside file");

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

    let link_target = fs::read_link(project.join("node_modules")).expect("read link");
    let rim_dir = link_target
        .parent()
        .and_then(Path::parent)
        .expect("rim dir");
    symlink(&outside_file, rim_dir.join("outside-big-file")).expect("external symlink");

    let out = Command::new(bin())
        .arg("status")
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("status");
    assert!(out.status.success());

    let stdout = String::from_utf8_lossy(&out.stdout);
    let size = stdout
        .lines()
        .find_map(|line| line.strip_prefix("rim_size_bytes: "))
        .and_then(|value| value.parse::<u64>().ok())
        .expect("rim_size_bytes");
    assert!(
        size < 200_000,
        "status should count the symlink itself, not the 1 MiB target; stdout: {stdout}"
    );
}

#[test]
fn gc_dry_run_and_orphaned_cleanup_use_metadata() {
    let project = make_project();
    let driver = make_project();
    let base = unique_temp("base");

    let prepare = Command::new(bin())
        .arg("prepare")
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("prepare");
    assert!(prepare.status.success());
    let link_target = fs::read_link(project.join("node_modules")).expect("read link");
    let rim_dir = link_target
        .parent()
        .and_then(Path::parent)
        .expect("rim dir")
        .to_path_buf();
    assert!(rim_dir.exists());

    fs::remove_dir_all(&project).expect("remove source project to orphan layer");

    let dry_run = Command::new(bin())
        .args(["gc", "--dry-run", "--orphaned"])
        .env("RIM_BASE", &base)
        .current_dir(&driver)
        .output()
        .expect("gc dry-run");
    assert!(dry_run.status.success());
    let stdout = String::from_utf8_lossy(&dry_run.stdout);
    assert!(stdout.contains("would remove"), "stdout: {stdout}");
    assert!(rim_dir.exists(), "dry-run should not remove layer");

    let gc = Command::new(bin())
        .args(["gc", "--orphaned"])
        .env("RIM_BASE", &base)
        .current_dir(&driver)
        .output()
        .expect("gc orphaned");
    assert!(gc.status.success());
    let stdout = String::from_utf8_lossy(&gc.stdout);
    assert!(stdout.contains("removed 1 layer"), "stdout: {stdout}");
    assert!(!rim_dir.exists(), "orphaned layer should be removed");
}

#[test]
fn npm_install_trims_cache_by_default_but_keeps_dependencies() {
    let project = make_project();
    let base = unique_temp("base");
    let fake_bin = unique_temp("bin");
    let fake_npm = fake_bin.join("npm");
    fs::write(
        &fake_npm,
        "#!/usr/bin/env bash\nset -euo pipefail\nmkdir -p \"$npm_config_cache\" node_modules/fake\nprintf cache > \"$npm_config_cache/blob\"\nprintf '{\"lockfileVersion\":3}\n' > package-lock.json\n",
    )
    .expect("fake npm");
    assert!(
        Command::new("chmod")
            .arg("+x")
            .arg(&fake_npm)
            .status()
            .unwrap()
            .success()
    );
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
    let link_target = fs::read_link(project.join("node_modules")).expect("read link");
    let rim_dir = link_target
        .parent()
        .and_then(Path::parent)
        .expect("rim dir");
    assert!(
        link_target.join("fake").exists(),
        "dependencies should remain installed"
    );
    assert!(
        !rim_dir.join("npm-cache").exists(),
        "npm cache should be trimmed by default"
    );
}

#[test]
fn keep_cache_preserves_npm_cache_after_install() {
    let project = make_project();
    let base = unique_temp("base");
    let fake_bin = unique_temp("bin");
    let fake_npm = fake_bin.join("npm");
    fs::write(
        &fake_npm,
        "#!/usr/bin/env bash\nset -euo pipefail\nmkdir -p \"$npm_config_cache\" node_modules/fake\nprintf cache > \"$npm_config_cache/blob\"\nprintf '{\"lockfileVersion\":3}\n' > package-lock.json\n",
    )
    .expect("fake npm");
    assert!(
        Command::new("chmod")
            .arg("+x")
            .arg(&fake_npm)
            .status()
            .unwrap()
            .success()
    );
    let path = format!(
        "{}:{}",
        fake_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let out = Command::new(bin())
        .args(["--keep-cache", "npm", "install"])
        .env("RIM_BASE", &base)
        .env("PATH", path)
        .current_dir(&project)
        .output()
        .expect("run rim fake npm install keep-cache");

    assert!(
        out.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let link_target = fs::read_link(project.join("node_modules")).expect("read link");
    let rim_dir = link_target
        .parent()
        .and_then(Path::parent)
        .expect("rim dir");
    assert!(
        rim_dir.join("npm-cache/blob").exists(),
        "--keep-cache should preserve npm cache"
    );
}

#[test]
fn auto_clean_removes_layer_after_success() {
    let project = make_project();
    let base = unique_temp("base");

    let out = Command::new(bin())
        .args([
            "--auto-clean",
            "sh",
            "-c",
            "test -L node_modules && test -d \"$(readlink node_modules)\"",
        ])
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("run rim auto-clean sh");

    assert!(
        out.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !project.join("node_modules").exists(),
        "auto-clean should remove the project symlink"
    );
    assert!(
        !base
            .read_dir()
            .map(|mut it| it.next().is_some())
            .unwrap_or(false),
        "auto-clean should remove the current rim dir"
    );
}

#[test]
fn auto_clean_keep_on_error_preserves_layer_and_exit_code() {
    let project = make_project();
    let base = unique_temp("base");

    let out = Command::new(bin())
        .args(["--auto-clean", "--keep-on-error", "sh", "-c", "exit 42"])
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("run rim auto-clean failing sh");

    assert_eq!(out.status.code(), Some(42));
    assert!(
        project.join("node_modules").is_symlink(),
        "--keep-on-error should preserve the dependency layer after failure"
    );
}

#[test]
fn auto_clean_install_warns_keeps_manifests_and_removes_dependencies() {
    let project = make_project();
    let base = unique_temp("base");
    let fake_bin = unique_temp("bin");
    let fake_npm = fake_bin.join("npm");
    fs::write(
        &fake_npm,
        "#!/usr/bin/env bash\nset -euo pipefail\nprintf '{\"lockfileVersion\":3}\n' > package-lock.json\nmkdir -p node_modules/fake\n",
    )
    .expect("fake npm");
    assert!(
        Command::new("chmod")
            .arg("+x")
            .arg(&fake_npm)
            .status()
            .unwrap()
            .success()
    );
    let path = format!(
        "{}:{}",
        fake_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let out = Command::new(bin())
        .args(["--auto-clean", "npm", "install"])
        .env("RIM_BASE", &base)
        .env("PATH", path)
        .current_dir(&project)
        .output()
        .expect("run rim auto-clean install");

    assert!(
        out.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("cleanup after install will remove installed dependencies"),
        "stderr: {stderr}"
    );
    assert!(
        project.join("package-lock.json").exists(),
        "manifest/lockfile changes should remain after auto-clean install"
    );
    assert!(
        !project.join("node_modules").exists(),
        "installed dependencies should be removed after auto-clean install"
    );
}

#[test]
fn ephemeral_installs_before_run_preserves_exit_code_and_cleans() {
    let project = make_project();
    let base = unique_temp("base");
    let fake_bin = unique_temp("bin");
    let fake_npm = fake_bin.join("npm");
    let log = project.join("npm-log.txt");
    fs::write(
        &fake_npm,
        "#!/usr/bin/env bash\nset -euo pipefail\necho \"$PWD|$*\" >> \"$RIM_TEST_LOG\"\nif [ \"${1:-}\" = install ]; then\n  printf '{\"lockfileVersion\":3}\n' > package-lock.json\n  mkdir -p node_modules/fake\n  exit 0\nfi\nif [ \"${1:-}\" = run ]; then\n  test -L node_modules\n  test -d \"$(readlink node_modules)/fake\"\n  exit 7\nfi\nexit 99\n",
    )
    .expect("fake npm");
    assert!(
        Command::new("chmod")
            .arg("+x")
            .arg(&fake_npm)
            .status()
            .unwrap()
            .success()
    );
    let path = format!(
        "{}:{}",
        fake_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let out = Command::new(bin())
        .args(["--ephemeral", "npm", "run", "dev"])
        .env("RIM_BASE", &base)
        .env("PATH", path)
        .env("RIM_TEST_LOG", &log)
        .current_dir(&project)
        .output()
        .expect("run rim ephemeral npm run");

    assert_eq!(
        out.status.code(),
        Some(7),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let log = fs::read_to_string(log).expect("npm log");
    assert!(log.contains("|install"), "log: {log}");
    assert!(log.contains("|run dev"), "log: {log}");
    assert!(
        project.join("package-lock.json").exists(),
        "ephemeral install should still copy lockfiles back"
    );
    assert!(
        !project.join("node_modules").exists(),
        "ephemeral should clean even when the requested command fails"
    );
}

#[test]
fn clean_cache_only_preserves_dependencies_and_removes_cache_dirs() {
    let project = make_project();
    let base = unique_temp("base");

    let prepare = Command::new(bin())
        .arg("prepare")
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("prepare");
    assert!(prepare.status.success());
    let link_target = fs::read_link(project.join("node_modules")).expect("read link");
    let rim_dir = link_target
        .parent()
        .and_then(Path::parent)
        .expect("rim dir");
    fs::create_dir_all(rim_dir.join("npm-cache")).expect("npm cache");
    fs::write(rim_dir.join("npm-cache/blob"), "cache").expect("cache blob");
    fs::create_dir_all(link_target.join("fake-pkg")).expect("fake dep");

    let clean = Command::new(bin())
        .args(["clean", "--cache-only"])
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("clean cache-only");
    assert!(clean.status.success());
    assert!(
        project.join("node_modules").is_symlink(),
        "deps symlink should remain"
    );
    assert!(
        link_target.join("fake-pkg").exists(),
        "dependencies should remain"
    );
    assert!(
        !rim_dir.join("npm-cache").exists(),
        "cache should be removed"
    );
}

#[test]
fn clean_deps_only_preserves_cache_and_metadata() {
    let project = make_project();
    let base = unique_temp("base");

    let prepare = Command::new(bin())
        .arg("prepare")
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("prepare");
    assert!(prepare.status.success());
    let link_target = fs::read_link(project.join("node_modules")).expect("read link");
    let rim_dir = link_target
        .parent()
        .and_then(Path::parent)
        .expect("rim dir");
    fs::create_dir_all(rim_dir.join("npm-cache")).expect("npm cache");
    fs::write(rim_dir.join("npm-cache/blob"), "cache").expect("cache blob");
    fs::create_dir_all(link_target.join("fake-pkg")).expect("fake dep");

    let clean = Command::new(bin())
        .args(["clean", "--deps-only"])
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("clean deps-only");
    assert!(clean.status.success());
    assert!(
        !project.join("node_modules").exists(),
        "project symlink should be removed"
    );
    assert!(!link_target.exists(), "dependency tree should be removed");
    assert!(
        rim_dir.join("npm-cache/blob").exists(),
        "cache should remain"
    );
    assert!(
        rim_dir.join(".rim-meta.json").exists(),
        "metadata should remain"
    );
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

#[test]
fn scan_detects_unmanaged_node_modules_candidates() {
    let root = unique_temp("scan-root");
    let project = root.join("tiny-hono");
    fs::create_dir_all(project.join("node_modules/pkg")).expect("node_modules");
    fs::write(
        project.join("package.json"),
        "{\"dependencies\":{\"hono\":\"4.0.0\"}}\n",
    )
    .expect("package");
    fs::write(project.join("bun.lock"), "").expect("bun lock");
    fs::write(
        project.join("node_modules/pkg/index.js"),
        "module.exports = 1\n",
    )
    .expect("dep");

    let out = Command::new(bin())
        .args(["scan", root.to_str().unwrap()])
        .env("RIM_BASE", unique_temp("base"))
        .current_dir(&root)
        .output()
        .expect("rim scan");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("tiny-hono"), "stdout: {stdout}");
    assert!(stdout.contains("bun"), "stdout: {stdout}");
    assert!(stdout.contains("low"), "stdout: {stdout}");
    assert!(stdout.contains("adoptable"), "stdout: {stdout}");
}

#[test]
fn scan_json_reports_pnpm_high_risk() {
    let root = unique_temp("scan-json-root");
    let project = root.join("pnpm-mono");
    fs::create_dir_all(project.join("node_modules/pkg")).expect("node_modules");
    fs::write(
        project.join("package.json"),
        "{\"workspaces\":[\"packages/*\"]}\n",
    )
    .expect("package");
    fs::write(project.join("pnpm-lock.yaml"), "lockfileVersion: 9\n").expect("pnpm lock");

    let out = Command::new(bin())
        .args(["scan", "--json", root.to_str().unwrap()])
        .env("RIM_BASE", unique_temp("base"))
        .current_dir(&root)
        .output()
        .expect("rim scan json");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.trim_start().starts_with('{'), "stdout: {stdout}");
    assert!(stdout.contains("\"candidates\""), "stdout: {stdout}");
    assert!(stdout.contains("\"manager\":\"pnpm\""), "stdout: {stdout}");
    assert!(stdout.contains("\"risk\":\"high\""), "stdout: {stdout}");
    assert!(stdout.contains("\"action\":\"skip\""), "stdout: {stdout}");
}

#[test]
fn scan_marks_managed_node_modules_symlink_as_skip() {
    let root = unique_temp("scan-managed-root");
    let project = root.join("managed-app");
    fs::create_dir_all(&project).expect("project");
    fs::write(project.join("package.json"), "{}\n").expect("package");
    let base = unique_temp("base");
    let prepare = Command::new(bin())
        .arg("prepare")
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("prepare");
    assert!(prepare.status.success());

    let out = Command::new(bin())
        .args(["scan", root.to_str().unwrap()])
        .env("RIM_BASE", &base)
        .current_dir(&root)
        .output()
        .expect("rim scan");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("managed-app"), "stdout: {stdout}");
    assert!(stdout.contains("skip"), "stdout: {stdout}");
    assert!(
        stdout.contains("already managed by rim"),
        "stdout: {stdout}"
    );
}

#[test]
fn adopt_dry_run_does_not_modify_node_modules() {
    let project = unique_temp("adopt-dry-project");
    let base = unique_temp("base");
    fs::write(project.join("package.json"), "{}\n").expect("package");
    fs::write(project.join("bun.lock"), "").expect("bun lock");
    fs::create_dir_all(project.join("node_modules/pkg")).expect("node_modules");
    fs::write(project.join("node_modules/pkg/index.js"), "original\n").expect("file");

    let out = Command::new(bin())
        .args(["adopt", project.to_str().unwrap(), "--dry-run"])
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("adopt dry-run");
    assert!(out.status.success());
    assert!(
        project.join("node_modules").is_dir(),
        "dry-run should keep real dir"
    );
    assert!(
        !project.join("node_modules").is_symlink(),
        "dry-run should not symlink"
    );
    assert!(project.join("node_modules/pkg/index.js").exists());
}

#[test]
fn adopt_moves_node_modules_to_layer_and_writes_metadata() {
    let project = unique_temp("adopt-project");
    let base = unique_temp("base");
    fs::write(project.join("package.json"), "{}\n").expect("package");
    fs::write(project.join("bun.lock"), "").expect("bun lock");
    fs::create_dir_all(project.join("node_modules/pkg")).expect("node_modules");
    fs::write(project.join("node_modules/pkg/index.js"), "original\n").expect("file");

    let out = Command::new(bin())
        .args(["adopt", project.to_str().unwrap()])
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("adopt");
    assert!(
        out.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(project.join("node_modules").is_symlink());
    let target = fs::read_link(project.join("node_modules")).expect("link");
    assert!(target.starts_with(&base), "target: {target:?}");
    assert!(target.join("pkg/index.js").exists());
    let rim_dir = target.parent().and_then(Path::parent).expect("rim dir");
    let meta = fs::read_to_string(rim_dir.join(".rim-meta.json")).expect("meta");
    assert!(meta.contains("\"adopted\": true"), "meta: {meta}");
    assert!(meta.contains("\"manager\": \"bun\""), "meta: {meta}");
}

#[test]
fn adopt_refuses_high_risk_pnpm_without_allow_risk() {
    let project = unique_temp("adopt-pnpm-project");
    let base = unique_temp("base");
    fs::write(project.join("package.json"), "{}\n").expect("package");
    fs::write(project.join("pnpm-lock.yaml"), "lockfileVersion: 9\n").expect("pnpm lock");
    fs::create_dir_all(project.join("node_modules/pkg")).expect("node_modules");

    let out = Command::new(bin())
        .args(["adopt", project.to_str().unwrap()])
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("adopt pnpm");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("high-risk adopt refused"),
        "stderr: {stderr}"
    );
}

#[test]
fn adopt_allows_high_risk_with_allow_risk() {
    let project = unique_temp("adopt-pnpm-allow-project");
    let base = unique_temp("base");
    fs::write(project.join("package.json"), "{}\n").expect("package");
    fs::write(project.join("pnpm-lock.yaml"), "lockfileVersion: 9\n").expect("pnpm lock");
    fs::create_dir_all(project.join("node_modules/pkg")).expect("node_modules");
    fs::write(project.join("node_modules/pkg/index.js"), "patched\n").expect("dep");

    let out = Command::new(bin())
        .args(["adopt", project.to_str().unwrap(), "--allow-risk"])
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("adopt pnpm allow");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(project.join("node_modules").is_symlink());
}

#[test]
fn adopt_warns_when_base_is_tmpfs() {
    if !Path::new("/dev/shm").exists() {
        return;
    }
    let project = unique_temp("adopt-tmpfs-project");
    let base = Path::new("/dev/shm").join(format!("rim-test-base-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    fs::write(project.join("package.json"), "{}\n").expect("package");
    fs::write(project.join("bun.lock"), "").expect("bun lock");
    fs::create_dir_all(project.join("node_modules/pkg")).expect("node_modules");

    let out = Command::new(bin())
        .args(["adopt", project.to_str().unwrap(), "--dry-run"])
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("adopt tmpfs dry-run");
    let _ = fs::remove_dir_all(&base);
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("adopting into tmpfs"), "stderr: {stderr}");
}

#[test]
fn diff_backup_saves_delta_and_restore_replays_changed_and_added_files() {
    let project = unique_temp("diff-backup-project");
    let base = unique_temp("base");
    fs::write(project.join("package.json"), "{}\n").expect("package");
    fs::write(project.join("bun.lock"), "").expect("bun lock");
    fs::create_dir_all(project.join("node_modules/pkg")).expect("node_modules");
    fs::write(project.join("node_modules/pkg/index.js"), "patched\n").expect("changed");
    fs::write(project.join("node_modules/pkg/local.js"), "local\n").expect("added");

    let fake_bin = unique_temp("bin");
    let fake_bun = fake_bin.join("bun");
    fs::write(
        &fake_bun,
        "#!/usr/bin/env bash\nset -euo pipefail\nmkdir -p node_modules/pkg\nprintf baseline > node_modules/pkg/index.js\nprintf lock > bun.lock\n",
    )
    .expect("fake bun");
    assert!(
        Command::new("chmod")
            .arg("+x")
            .arg(&fake_bun)
            .status()
            .unwrap()
            .success()
    );
    let path = format!(
        "{}:{}",
        fake_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let out = Command::new(bin())
        .args(["adopt", project.to_str().unwrap(), "--diff-backup"])
        .env("RIM_BASE", &base)
        .env("PATH", &path)
        .current_dir(&project)
        .output()
        .expect("adopt diff backup");
    assert!(
        out.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let backup_root = project.join(".rim-backups");
    assert!(backup_root.exists());
    let backups = fs::read_dir(&backup_root)
        .expect("backups")
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(backups.len(), 1);
    let backup = backups[0].path();
    assert!(
        backup.join("changed/node_modules/pkg/index.js").exists(),
        "changed file backup missing"
    );
    assert!(
        backup.join("added/node_modules/pkg/local.js").exists(),
        "added file backup missing"
    );

    fs::write(project.join("node_modules/pkg/index.js"), "baseline\n").expect("overwrite changed");
    fs::remove_file(project.join("node_modules/pkg/local.js")).expect("remove added");

    let dry = Command::new(bin())
        .args(["backup", "restore", "latest", "--dry-run"])
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("restore dry-run");
    assert!(dry.status.success());
    assert!(
        !project.join("node_modules/pkg/local.js").exists(),
        "dry-run should not restore"
    );

    let restore = Command::new(bin())
        .args(["backup", "restore", "latest"])
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("restore");
    assert!(
        restore.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&restore.stderr)
    );
    assert_eq!(
        fs::read_to_string(project.join("node_modules/pkg/index.js")).unwrap(),
        "patched\n"
    );
    assert_eq!(
        fs::read_to_string(project.join("node_modules/pkg/local.js")).unwrap(),
        "local\n"
    );

    let list = Command::new(bin())
        .args(["backup", "list"])
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("backup list");
    assert!(list.status.success());
    assert!(String::from_utf8_lossy(&list.stdout).contains("node_modules-delta"));

    let show = Command::new(bin())
        .args(["backup", "show", "latest"])
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("backup show");
    assert!(show.status.success());
    assert!(String::from_utf8_lossy(&show.stdout).contains("changed:"));
}

#[test]
fn scan_diff_detects_manual_difference_without_adopting() {
    let project = unique_temp("scan-diff-project");
    let base = unique_temp("base");
    fs::write(project.join("package.json"), "{}\n").expect("package");
    fs::write(project.join("bun.lock"), "").expect("bun lock");
    fs::create_dir_all(project.join("node_modules/pkg")).expect("node_modules");
    fs::write(project.join("node_modules/pkg/index.js"), "patched\n").expect("changed");

    let fake_bin = unique_temp("bin");
    let fake_bun = fake_bin.join("bun");
    fs::write(
        &fake_bun,
        "#!/usr/bin/env bash\nset -euo pipefail\nmkdir -p node_modules/pkg\nprintf baseline\\n > node_modules/pkg/index.js\nprintf lock > bun.lock\n",
    )
    .expect("fake bun");
    assert!(
        Command::new("chmod")
            .arg("+x")
            .arg(&fake_bun)
            .status()
            .unwrap()
            .success()
    );
    let path = format!(
        "{}:{}",
        fake_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let out = Command::new(bin())
        .args(["scan", project.to_str().unwrap(), "--diff"])
        .env("RIM_BASE", &base)
        .env("PATH", path)
        .current_dir(&project)
        .output()
        .expect("scan diff");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("manual_diff: detected"), "stdout: {stdout}");
    assert!(stdout.contains("changed=1"), "stdout: {stdout}");
    assert!(
        project.join("node_modules").is_dir(),
        "scan --diff should not adopt"
    );
}

#[test]
fn scan_json_diff_outputs_diff_in_stdout_json_only() {
    let project = unique_temp("scan-json-diff-project");
    let base = unique_temp("base");
    fs::write(project.join("package.json"), "{}\n").expect("package");
    fs::write(project.join("bun.lock"), "").expect("bun lock");
    fs::create_dir_all(project.join("node_modules/pkg")).expect("node_modules");
    fs::write(project.join("node_modules/pkg/index.js"), "patched\n").expect("changed");

    let fake_bin = unique_temp("bin");
    let fake_bun = fake_bin.join("bun");
    fs::write(
        &fake_bun,
        "#!/usr/bin/env bash\nset -euo pipefail\nmkdir -p node_modules/pkg\nprintf baseline\\n > node_modules/pkg/index.js\nprintf lock > bun.lock\n",
    )
    .expect("fake bun");
    assert!(
        Command::new("chmod")
            .arg("+x")
            .arg(&fake_bun)
            .status()
            .unwrap()
            .success()
    );
    let path = format!(
        "{}:{}",
        fake_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let out = Command::new(bin())
        .args(["scan", "--json", "--diff", project.to_str().unwrap()])
        .env("RIM_BASE", &base)
        .env("PATH", path)
        .current_dir(&project)
        .output()
        .expect("scan json diff");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).trim().is_empty(),
        "stderr should be empty for JSON: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.trim_start().starts_with('{'), "stdout: {stdout}");
    assert!(
        stdout.contains("\"manual_diff\": \"detected\""),
        "stdout: {stdout}"
    );
    assert!(stdout.contains("\"diff\""), "stdout: {stdout}");
}

#[test]
fn adopt_diff_backup_dry_run_does_not_create_backup_or_scratch() {
    let project = unique_temp("adopt-diff-dry-project");
    let base = unique_temp("base");
    fs::write(project.join("package.json"), "{}\n").expect("package");
    fs::write(project.join("bun.lock"), "").expect("bun lock");
    fs::create_dir_all(project.join("node_modules/pkg")).expect("node_modules");
    fs::write(project.join("node_modules/pkg/index.js"), "patched\n").expect("dep");

    let out = Command::new(bin())
        .args([
            "adopt",
            project.to_str().unwrap(),
            "--diff-backup",
            "--dry-run",
        ])
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("adopt diff-backup dry-run");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("diff_backup: would compare"),
        "stdout: {stdout}"
    );
    assert!(project.join("node_modules").is_dir());
    assert!(
        !project.join(".rim-backups").exists(),
        "dry-run should not create backups"
    );
    assert!(
        fs::read_dir(&base).expect("base dir").next().is_none(),
        "dry-run should not create scratch/layer dirs under RIM_BASE"
    );
}

#[test]
fn backup_restore_apply_deletes_removes_deleted_entries_only_when_requested() {
    let project = unique_temp("backup-delete-project");
    let base = unique_temp("base");
    fs::write(project.join("package.json"), "{}\n").expect("package");
    fs::create_dir_all(project.join("node_modules/pkg")).expect("node_modules");
    fs::write(project.join("node_modules/pkg/remove.js"), "remove\n").expect("remove file");
    let backup = project.join(".rim-backups/node_modules-delta-1");
    fs::create_dir_all(&backup).expect("backup");
    fs::write(backup.join("summary.txt"), "deleted: 1\n").expect("summary");
    fs::write(
        backup.join("deleted.json"),
        "[\"node_modules/pkg/remove.js\"]\n",
    )
    .expect("deleted");

    let dry = Command::new(bin())
        .args([
            "backup",
            "restore",
            "latest",
            "--dry-run",
            "--apply-deletes",
        ])
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("restore dry apply deletes");
    assert!(dry.status.success());
    assert!(
        project.join("node_modules/pkg/remove.js").exists(),
        "dry-run should not delete"
    );

    let no_delete = Command::new(bin())
        .args(["backup", "restore", "latest"])
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("restore no deletes");
    assert!(no_delete.status.success());
    assert!(
        project.join("node_modules/pkg/remove.js").exists(),
        "default restore should not delete"
    );

    let delete = Command::new(bin())
        .args(["backup", "restore", "latest", "--apply-deletes"])
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("restore apply deletes");
    assert!(delete.status.success());
    assert!(
        !project.join("node_modules/pkg/remove.js").exists(),
        "--apply-deletes should delete"
    );
}

#[test]
fn run_command_writes_active_lock_while_child_is_running_and_removes_it_after() {
    let project = make_project();
    let base = unique_temp("base");
    fs::write(project.join("bun.lock"), "").expect("bun lock");
    let rim_dir = Command::new(bin())
        .arg("path")
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("rim path");
    assert!(rim_dir.status.success());
    let rim_dir = PathBuf::from(String::from_utf8_lossy(&rim_dir.stdout).trim().to_owned());

    let fake_bin = unique_temp("bin");
    let fake_bun = fake_bin.join("bun");
    fs::write(
        &fake_bun,
        "#!/usr/bin/env bash\nset -euo pipefail\nif [ -f \"$RIM_TEST_RIM_DIR/.rim-active\" ]; then printf active > \"$RIM_TEST_LOG\"; else printf missing > \"$RIM_TEST_LOG\"; fi\n",
    )
    .expect("fake bun");
    assert!(
        Command::new("chmod")
            .arg("+x")
            .arg(&fake_bun)
            .status()
            .unwrap()
            .success()
    );
    let log = project.join("active.log");
    let path = format!(
        "{}:{}",
        fake_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let out = Command::new(bin())
        .args(["bun", "run", "dev"])
        .env("RIM_BASE", &base)
        .env("PATH", path)
        .env("RIM_TEST_RIM_DIR", &rim_dir)
        .env("RIM_TEST_LOG", &log)
        .current_dir(&project)
        .output()
        .expect("rim bun run dev");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(fs::read_to_string(log).expect("log"), "active");
    assert!(
        !rim_dir.join(".rim-active").exists(),
        "active lock should be removed after command exits"
    );
}

#[test]
fn clean_refuses_active_layer_unless_force() {
    let project = make_project();
    let base = unique_temp("base");
    let prepare = Command::new(bin())
        .arg("prepare")
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("prepare");
    assert!(prepare.status.success());
    let link_target = fs::read_link(project.join("node_modules")).expect("read link");
    let rim_dir = link_target
        .parent()
        .and_then(Path::parent)
        .expect("rim dir");
    fs::write(
        rim_dir.join(".rim-active"),
        format!(
            "{{\"pid\":{},\"started_at\":0,\"command\":\"test\"}}\n",
            std::process::id()
        ),
    )
    .expect("active lock");

    let clean = Command::new(bin())
        .arg("clean")
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("clean active");
    assert!(!clean.status.success());
    let stderr = String::from_utf8_lossy(&clean.stderr);
    assert!(stderr.contains("rim layer is active"), "stderr: {stderr}");
    assert!(rim_dir.exists(), "active layer should remain");

    let clean = Command::new(bin())
        .args(["clean", "--force"])
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("clean force");
    assert!(
        clean.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&clean.stderr)
    );
    assert!(!rim_dir.exists(), "--force should remove active layer");
}

#[test]
fn gc_skips_active_layer_unless_force() {
    let project = make_project();
    let driver = make_project();
    let base = unique_temp("base");
    let prepare = Command::new(bin())
        .arg("prepare")
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("prepare");
    assert!(prepare.status.success());
    let link_target = fs::read_link(project.join("node_modules")).expect("read link");
    let rim_dir = link_target
        .parent()
        .and_then(Path::parent)
        .expect("rim dir")
        .to_path_buf();
    fs::write(
        rim_dir.join(".rim-active"),
        format!(
            "{{\"pid\":{},\"started_at\":0,\"command\":\"test\"}}\n",
            std::process::id()
        ),
    )
    .expect("active lock");

    let gc = Command::new(bin())
        .args(["gc", "--all"])
        .env("RIM_BASE", &base)
        .current_dir(&driver)
        .output()
        .expect("gc all");
    assert!(gc.status.success());
    let stdout = String::from_utf8_lossy(&gc.stdout);
    assert!(stdout.contains("skipping active layer"), "stdout: {stdout}");
    assert!(rim_dir.exists(), "active layer should survive gc --all");

    let gc = Command::new(bin())
        .args(["gc", "--all", "--force"])
        .env("RIM_BASE", &base)
        .current_dir(&driver)
        .output()
        .expect("gc all force");
    assert!(
        gc.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&gc.stderr)
    );
    assert!(!rim_dir.exists(), "gc --force should remove active layer");
}

#[test]
fn ls_marks_stale_active_lock() {
    let project = make_project();
    let base = unique_temp("base");
    let prepare = Command::new(bin())
        .arg("prepare")
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("prepare");
    assert!(prepare.status.success());
    let link_target = fs::read_link(project.join("node_modules")).expect("read link");
    let rim_dir = link_target
        .parent()
        .and_then(Path::parent)
        .expect("rim dir");
    fs::write(
        rim_dir.join(".rim-active"),
        "{\"pid\":99999999,\"started_at\":0,\"command\":\"dead\"}\n",
    )
    .expect("stale lock");

    let out = Command::new(bin())
        .arg("ls")
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("rim ls");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("ACTIVE"), "stdout: {stdout}");
    assert!(stdout.contains("stale:99999999"), "stdout: {stdout}");
}

#[test]
fn repair_stale_locks_dry_run_and_apply() {
    let project = make_project();
    let base = unique_temp("base");
    let prepare = Command::new(bin())
        .arg("prepare")
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("prepare");
    assert!(prepare.status.success());
    let link_target = fs::read_link(project.join("node_modules")).expect("read link");
    let rim_dir = link_target
        .parent()
        .and_then(Path::parent)
        .expect("rim dir");
    let lock = rim_dir.join(".rim-active");
    fs::write(
        &lock,
        "{\"pid\":99999999,\"started_at\":0,\"command\":\"dead\"}\n",
    )
    .expect("stale lock");

    let dry = Command::new(bin())
        .args(["repair", "--stale-locks", "--dry-run"])
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("repair dry-run");
    assert!(dry.status.success());
    let stdout = String::from_utf8_lossy(&dry.stdout);
    assert!(
        stdout.contains("would remove stale lock"),
        "stdout: {stdout}"
    );
    assert!(lock.exists(), "dry-run should leave stale lock");

    let repair = Command::new(bin())
        .args(["repair", "--stale-locks"])
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("repair");
    assert!(repair.status.success());
    let stdout = String::from_utf8_lossy(&repair.stdout);
    assert!(stdout.contains("removed stale lock"), "stdout: {stdout}");
    assert!(!lock.exists(), "repair should remove stale lock");
}

#[test]
fn repair_does_not_remove_live_active_lock() {
    let project = make_project();
    let base = unique_temp("base");
    let prepare = Command::new(bin())
        .arg("prepare")
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("prepare");
    assert!(prepare.status.success());
    let link_target = fs::read_link(project.join("node_modules")).expect("read link");
    let rim_dir = link_target
        .parent()
        .and_then(Path::parent)
        .expect("rim dir");
    let lock = rim_dir.join(".rim-active");
    fs::write(
        &lock,
        format!(
            "{{\"pid\":{},\"started_at\":0,\"command\":\"live\"}}\n",
            std::process::id()
        ),
    )
    .expect("live lock");

    let repair = Command::new(bin())
        .args(["repair", "--stale-locks"])
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("repair");
    assert!(repair.status.success());
    let stdout = String::from_utf8_lossy(&repair.stdout);
    assert!(stdout.contains("removed 0 stale lock"), "stdout: {stdout}");
    assert!(lock.exists(), "live lock should remain");
}

#[test]
fn doctor_suggest_reports_stale_locks() {
    let project = make_project();
    let base = unique_temp("base");
    let prepare = Command::new(bin())
        .arg("prepare")
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("prepare");
    assert!(prepare.status.success());
    let link_target = fs::read_link(project.join("node_modules")).expect("read link");
    let rim_dir = link_target
        .parent()
        .and_then(Path::parent)
        .expect("rim dir");
    fs::write(rim_dir.join(".rim-active"), "{\"pid\":99999999}\n").expect("stale lock");

    let doctor = Command::new(bin())
        .args(["doctor", "--suggest"])
        .env("RIM_BASE", &base)
        .current_dir(&project)
        .output()
        .expect("doctor suggest");
    assert!(doctor.status.success());
    let stdout = String::from_utf8_lossy(&doctor.stdout);
    assert!(stdout.contains("stale active lock"), "stdout: {stdout}");
    assert!(
        stdout.contains("rim repair --stale-locks --dry-run"),
        "stdout: {stdout}"
    );
}
