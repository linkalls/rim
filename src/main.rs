use std::env;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::fs::symlink;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone)]
struct RimContext {
    project_root: PathBuf,
    rim_base: PathBuf,
    rim_dir: PathBuf,
    shadow_project: PathBuf,
    node_modules: PathBuf,
    npm_cache: PathBuf,
    xdg_cache: PathBuf,
    tmp: PathBuf,
    deno_dir: PathBuf,
    playwright_browsers: PathBuf,
    bun_cache: PathBuf,
    pnpm_store: PathBuf,
}

#[derive(Debug, Clone, Copy, Default)]
struct CliOptions {
    dry_run: bool,
    auto_clean: bool,
    keep_on_error: bool,
    ephemeral: bool,
}

impl CliOptions {
    fn should_clean_after(self, exit_code: u8) -> bool {
        (self.auto_clean || self.ephemeral) && (!self.keep_on_error || exit_code == 0)
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code),
        Err(err) => {
            eprintln!("rim: {err}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<u8, String> {
    let mut args: Vec<OsString> = env::args_os().skip(1).collect();
    let options = parse_options(&mut args)?;

    let Some(command) = args.first().and_then(|a| a.to_str()).map(str::to_owned) else {
        print_help();
        return Ok(2);
    };

    let ctx = build_context()?;

    match command.as_str() {
        "prepare" => {
            ensure_layout(&ctx)?;
            sync_manifests_to_shadow(&ctx)?;
            print_context(&ctx);
            Ok(0)
        }
        "clean" => {
            clean(&ctx)?;
            println!("cleaned: {}", ctx.rim_dir.display());
            Ok(0)
        }
        "status" => {
            status(&ctx);
            Ok(0)
        }
        "doctor" => {
            doctor(&ctx);
            Ok(0)
        }
        "help" | "--help" | "-h" => {
            print_help();
            Ok(0)
        }
        tool => {
            let tool_args = args.split_off(1);
            run_tool(&ctx, tool, tool_args, options)
        }
    }
}

fn parse_options(args: &mut Vec<OsString>) -> Result<CliOptions, String> {
    let mut options = CliOptions::default();
    loop {
        let Some(flag) = args.first().and_then(|arg| arg.to_str()) else {
            break;
        };
        match flag {
            "--dry-run" => options.dry_run = true,
            "--auto-clean" => options.auto_clean = true,
            "--keep-on-error" => options.keep_on_error = true,
            "--ephemeral" => {
                options.ephemeral = true;
                options.auto_clean = true;
            }
            _ => break,
        }
        args.remove(0);
    }
    if options.keep_on_error && !(options.auto_clean || options.ephemeral) {
        return Err("--keep-on-error requires --auto-clean or --ephemeral".to_owned());
    }
    Ok(options)
}

fn print_help() {
    println!(
        "rim - RAM dependency wrapper\n\nUsage:\n  rim prepare\n  rim status\n  rim doctor\n  rim clean\n  rim [--dry-run] <bun|npm|pnpm|deno|node|...> [args...]\n\nEnvironment:\n  RIM_BASE   dependency layer base directory, default /dev/shm/rim"
    );
}

fn build_context() -> Result<RimContext, String> {
    let cwd = env::current_dir().map_err(|e| format!("cannot read cwd: {e}"))?;
    let project_root = find_project_root(&cwd);
    let base = env::var_os("RIM_BASE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/dev/shm/rim"));
    let name = project_root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("project");
    let hash = short_hash(&project_root);
    let rim_dir = base.join(format!("{name}-{hash}"));
    let shadow_project = rim_dir.join("project");

    Ok(RimContext {
        project_root,
        rim_base: base,
        node_modules: shadow_project.join("node_modules"),
        npm_cache: rim_dir.join("npm-cache"),
        xdg_cache: rim_dir.join("xdg-cache"),
        tmp: rim_dir.join("tmp"),
        deno_dir: rim_dir.join("deno-cache"),
        playwright_browsers: rim_dir.join("playwright-browsers"),
        bun_cache: rim_dir.join("bun-cache"),
        pnpm_store: rim_dir.join("pnpm-store"),
        shadow_project,
        rim_dir,
    })
}

fn find_project_root(start: &Path) -> PathBuf {
    let markers = [
        "package.json",
        "deno.json",
        "deno.jsonc",
        "bun.lock",
        "bun.lockb",
        "pnpm-lock.yaml",
        "package-lock.json",
        "yarn.lock",
        ".git",
    ];

    for dir in start.ancestors() {
        if markers.iter().any(|marker| dir.join(marker).exists()) {
            return dir.to_path_buf();
        }
    }

    start.to_path_buf()
}

fn short_hash(path: &Path) -> String {
    // Stable FNV-1a 64-bit hash. This is for deterministic directory names,
    // not for cryptographic identity.
    let mut hash = 0xcbf29ce484222325u64;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")[..8].to_owned()
}

fn ensure_layout(ctx: &RimContext) -> Result<(), String> {
    for dir in [
        &ctx.rim_dir,
        &ctx.shadow_project,
        &ctx.node_modules,
        &ctx.npm_cache,
        &ctx.xdg_cache,
        &ctx.tmp,
        &ctx.deno_dir,
        &ctx.playwright_browsers,
        &ctx.bun_cache,
        &ctx.pnpm_store,
    ] {
        fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    fs::write(ctx.node_modules.join(".rim-keep"), b"")
        .map_err(|e| format!("cannot write .rim-keep: {e}"))?;
    ensure_node_modules_link(ctx)
}

fn sync_manifests_to_shadow(ctx: &RimContext) -> Result<(), String> {
    fs::create_dir_all(&ctx.shadow_project)
        .map_err(|e| format!("cannot create shadow project: {e}"))?;

    for name in manifest_names() {
        let stale = ctx.shadow_project.join(name);
        if stale.exists() {
            fs::remove_file(&stale).map_err(|e| {
                format!(
                    "cannot remove stale shadow manifest {}: {e}",
                    stale.display()
                )
            })?;
        }
    }

    for name in manifest_names() {
        let src = ctx.project_root.join(name);
        let dst = ctx.shadow_project.join(name);
        if src.exists() {
            fs::copy(&src, &dst)
                .map_err(|e| format!("cannot copy {} to shadow project: {e}", src.display()))?;
        }
    }
    Ok(())
}

fn sync_mutated_manifests_back(ctx: &RimContext) -> Result<(), String> {
    for name in mutable_manifest_names() {
        let src = ctx.shadow_project.join(name);
        let dst = ctx.project_root.join(name);
        if src.exists() {
            fs::copy(&src, &dst)
                .map_err(|e| format!("cannot copy {} back to project: {e}", src.display()))?;
        }
    }
    Ok(())
}

fn manifest_names() -> &'static [&'static str] {
    &[
        "package.json",
        "package-lock.json",
        "npm-shrinkwrap.json",
        "pnpm-lock.yaml",
        "bun.lock",
        "bun.lockb",
        "yarn.lock",
        ".npmrc",
    ]
}

fn mutable_manifest_names() -> &'static [&'static str] {
    &[
        "package.json",
        "package-lock.json",
        "npm-shrinkwrap.json",
        "pnpm-lock.yaml",
        "bun.lock",
        "bun.lockb",
        "yarn.lock",
    ]
}

fn ensure_node_modules_link(ctx: &RimContext) -> Result<(), String> {
    let link = ctx.project_root.join("node_modules");

    match fs::symlink_metadata(&link) {
        Ok(meta) if meta.file_type().is_symlink() => {
            let target = fs::read_link(&link)
                .map_err(|e| format!("cannot read node_modules symlink: {e}"))?;
            if target != ctx.node_modules {
                fs::remove_file(&link)
                    .map_err(|e| format!("cannot replace node_modules symlink: {e}"))?;
                symlink(&ctx.node_modules, &link)
                    .map_err(|e| format!("cannot create node_modules symlink: {e}"))?;
            }
            Ok(())
        }
        Ok(_) => Err(
            "node_modules exists and is not a symlink; move or delete it before using rim"
                .to_owned(),
        ),
        Err(e) if e.kind() == io::ErrorKind::NotFound => symlink(&ctx.node_modules, &link)
            .map_err(|e| format!("cannot create node_modules symlink: {e}")),
        Err(e) => Err(format!("cannot inspect node_modules: {e}")),
    }
}

fn clean(ctx: &RimContext) -> Result<(), String> {
    let link = ctx.project_root.join("node_modules");
    if fs::symlink_metadata(&link)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        let target =
            fs::read_link(&link).map_err(|e| format!("cannot read node_modules link: {e}"))?;
        if target == ctx.node_modules {
            fs::remove_file(&link)
                .map_err(|e| format!("cannot remove node_modules symlink: {e}"))?;
        }
    }

    if ctx.rim_dir.exists() {
        fs::remove_dir_all(&ctx.rim_dir)
            .map_err(|e| format!("cannot remove {}: {e}", ctx.rim_dir.display()))?;
    }
    Ok(())
}

fn status(ctx: &RimContext) {
    println!("project: {}", ctx.project_root.display());
    println!("rim_base: {}", ctx.rim_base.display());
    println!("rim_dir: {}", ctx.rim_dir.display());
    println!("shadow_project: {}", ctx.shadow_project.display());
    println!("node_modules: {}", ctx.node_modules.display());
    let bytes = dir_size(&ctx.rim_dir).unwrap_or(0);
    println!("rim_size_bytes: {bytes}");
}

#[derive(Debug, Clone)]
struct StorageInfo {
    path: PathBuf,
    mount_point: Option<PathBuf>,
    fs_type: Option<String>,
    total_bytes: u64,
    used_bytes: u64,
    available_bytes: u64,
}

#[derive(Debug, Clone, Default)]
struct MemoryInfo {
    total_bytes: Option<u64>,
    available_bytes: Option<u64>,
    shmem_bytes: Option<u64>,
}

fn doctor(ctx: &RimContext) {
    println!("project: {}", ctx.project_root.display());
    println!("rim_base: {}", ctx.rim_base.display());
    println!("rim_dir: {}", ctx.rim_dir.display());
    println!("mode: {}", rim_mode(ctx));

    println!();
    println!("storage:");
    print_storage("project", storage_info_for(&ctx.project_root));
    print_storage("rim_base", storage_info_for(&ctx.rim_base));

    println!();
    println!("memory:");
    let memory = read_memory_info();
    println!(
        "  total: {}",
        memory
            .total_bytes
            .map(format_bytes)
            .unwrap_or_else(|| "unknown".to_owned())
    );
    println!(
        "  available: {}",
        memory
            .available_bytes
            .map(format_bytes)
            .unwrap_or_else(|| "unknown".to_owned())
    );
    println!(
        "  shmem_used: {}",
        memory
            .shmem_bytes
            .map(format_bytes)
            .unwrap_or_else(|| "unknown".to_owned())
    );

    println!();
    println!("rim:");
    println!(
        "  current_project_usage: {}",
        format_bytes(dir_size(&ctx.rim_dir).unwrap_or(0))
    );
    println!(
        "  total_base_usage: {}",
        format_bytes(dir_size(&ctx.rim_base).unwrap_or(0))
    );

    println!();
    println!("risk:");
    println!("  install_risk: {}", install_risk(ctx));
    println!(
        "  workspace: {}",
        if workspace_detected(ctx) {
            "detected"
        } else {
            "not detected"
        }
    );
    println!(
        "  lifecycle_scripts: {}",
        if lifecycle_scripts_detected(ctx) {
            "detected"
        } else {
            "not detected"
        }
    );
}

fn print_storage(label: &str, info: Option<StorageInfo>) {
    match info {
        Some(info) => {
            println!("  {label}:");
            println!("    path: {}", info.path.display());
            if let Some(mount_point) = info.mount_point {
                println!("    mount: {}", mount_point.display());
            }
            if let Some(fs_type) = info.fs_type {
                println!("    fs: {fs_type}");
            }
            println!("    total: {}", format_bytes(info.total_bytes));
            println!("    used: {}", format_bytes(info.used_bytes));
            println!("    available: {}", format_bytes(info.available_bytes));
        }
        None => {
            println!("  {label}: unknown");
        }
    }
}

fn warn_about_low_rim_space(ctx: &RimContext) {
    let risk = install_risk(ctx);
    if matches!(risk, "medium" | "high")
        && let Some(info) = storage_info_for(&ctx.rim_base)
    {
        eprintln!(
            "rim: warning: RIM_BASE has {} available ({risk} install risk): {}",
            format_bytes(info.available_bytes),
            ctx.rim_base.display()
        );
    }
}

fn install_risk(ctx: &RimContext) -> &'static str {
    const HIGH: u64 = 512 * 1024 * 1024;
    const MEDIUM: u64 = 1024 * 1024 * 1024;
    match storage_info_for(&ctx.rim_base).map(|info| info.available_bytes) {
        Some(bytes) if bytes < HIGH => "high",
        Some(bytes) if bytes < MEDIUM => "medium",
        Some(_) => "low",
        None => "unknown",
    }
}

fn rim_mode(ctx: &RimContext) -> String {
    let fs_type = mount_info_for(&ctx.rim_base).and_then(|mount| mount.fs_type);
    if fs_type.as_deref() == Some("tmpfs") {
        "tmpfs".to_owned()
    } else if ctx.rim_base.starts_with(cache_dir()) {
        "cache".to_owned()
    } else {
        "disk".to_owned()
    }
}

fn cache_dir() -> PathBuf {
    env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(|| PathBuf::from(".cache"))
}

fn storage_info_for(path: &Path) -> Option<StorageInfo> {
    let existing = existing_ancestor(path)?;
    let output = Command::new("df").arg("-kP").arg(&existing).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.lines().nth(1)?;
    let fields = line.split_whitespace().collect::<Vec<_>>();
    if fields.len() < 6 {
        return None;
    }
    let total_kib = fields[1].parse::<u64>().ok()?;
    let used_kib = fields[2].parse::<u64>().ok()?;
    let available_kib = fields[3].parse::<u64>().ok()?;
    let mount = mount_info_for(&existing);
    Some(StorageInfo {
        path: path.to_path_buf(),
        mount_point: mount.as_ref().map(|m| m.mount_point.clone()),
        fs_type: mount.and_then(|m| m.fs_type),
        total_bytes: total_kib.saturating_mul(1024),
        used_bytes: used_kib.saturating_mul(1024),
        available_bytes: available_kib.saturating_mul(1024),
    })
}

#[derive(Debug, Clone)]
struct MountInfo {
    mount_point: PathBuf,
    fs_type: Option<String>,
}

fn mount_info_for(path: &Path) -> Option<MountInfo> {
    let existing = existing_ancestor(path)?;
    let mounts = fs::read_to_string("/proc/mounts").ok()?;
    let mut best: Option<MountInfo> = None;
    for line in mounts.lines() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 3 {
            continue;
        }
        let mount_point = PathBuf::from(unescape_mount_path(fields[1]));
        if existing.starts_with(&mount_point)
            && best.as_ref().is_none_or(|current| {
                mount_point.as_os_str().len() > current.mount_point.as_os_str().len()
            })
        {
            best = Some(MountInfo {
                mount_point,
                fs_type: Some(fields[2].to_owned()),
            });
        }
    }
    best
}

fn unescape_mount_path(path: &str) -> String {
    path.replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}

fn existing_ancestor(path: &Path) -> Option<PathBuf> {
    let mut candidate = path;
    loop {
        if candidate.exists() {
            return Some(candidate.to_path_buf());
        }
        candidate = candidate.parent()?;
    }
}

fn read_memory_info() -> MemoryInfo {
    let Ok(contents) = fs::read_to_string("/proc/meminfo") else {
        return MemoryInfo::default();
    };
    let mut info = MemoryInfo::default();
    for line in contents.lines() {
        let mut parts = line.split_whitespace();
        let Some(key) = parts.next() else { continue };
        let Some(value) = parts.next().and_then(|v| v.parse::<u64>().ok()) else {
            continue;
        };
        let bytes = value.saturating_mul(1024);
        match key.trim_end_matches(':') {
            "MemTotal" => info.total_bytes = Some(bytes),
            "MemAvailable" => info.available_bytes = Some(bytes),
            "Shmem" => info.shmem_bytes = Some(bytes),
            _ => {}
        }
    }
    info
}

fn workspace_detected(ctx: &RimContext) -> bool {
    [
        "pnpm-workspace.yaml",
        "turbo.json",
        "rush.json",
        "lerna.json",
    ]
    .iter()
    .any(|name| ctx.project_root.join(name).exists())
        || fs::read_to_string(ctx.project_root.join("package.json"))
            .map(|package_json| package_json.contains("\"workspaces\""))
            .unwrap_or(false)
}

fn lifecycle_scripts_detected(ctx: &RimContext) -> bool {
    let Ok(package_json) = fs::read_to_string(ctx.project_root.join("package.json")) else {
        return false;
    };
    [
        "\"preinstall\"",
        "\"install\"",
        "\"postinstall\"",
        "\"prepare\"",
    ]
    .iter()
    .any(|needle| package_json.contains(needle))
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = UNITS[0];
    for next_unit in UNITS {
        unit = next_unit;
        if value < 1024.0 || next_unit == "TB" {
            break;
        }
        value /= 1024.0;
    }
    if unit == "B" {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {unit}")
    }
}

fn dir_size(path: &Path) -> io::Result<u64> {
    if !path.exists() {
        return Ok(0);
    }
    let mut total = 0;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.is_dir() {
            total += dir_size(&entry.path())?;
        } else {
            total += meta.len();
        }
    }
    Ok(total)
}

fn run_tool(
    ctx: &RimContext,
    tool: &str,
    args: Vec<OsString>,
    options: CliOptions,
) -> Result<u8, String> {
    let install_like = is_install_like(tool, &args);
    if (options.auto_clean || options.ephemeral) && install_like {
        eprintln!("rim: warning: --auto-clean after install will remove installed dependencies.");
        eprintln!("rim: manifest and lockfile changes will remain.");
    }
    if install_like {
        warn_about_low_rim_space(ctx);
    }

    if options.ephemeral && !options.dry_run {
        clean(ctx)?;
    }

    ensure_layout(ctx)?;

    let needs_ephemeral_install = options.ephemeral
        && !install_like
        && should_ephemeral_install(tool, &args)
        && (options.dry_run || dependencies_missing(ctx));

    if needs_ephemeral_install {
        let install_code = run_install_like(ctx, tool, options.dry_run)?;
        if install_code != 0 {
            if options.should_clean_after(install_code) {
                clean_after_command(ctx);
            }
            return Ok(install_code);
        }
    }

    if install_like {
        sync_manifests_to_shadow(ctx)?;
    }

    let final_args = final_args(ctx, tool, args);
    let cwd = if install_like {
        &ctx.shadow_project
    } else {
        &ctx.project_root
    };

    if options.dry_run {
        println!("project: {}", ctx.project_root.display());
        println!("rim_base: {}", ctx.rim_base.display());
        println!("rim_dir: {}", ctx.rim_dir.display());
        println!("cwd: {}", cwd.display());
        println!("command: {} {}", tool, join_args(&final_args));
        println!("auto_clean={}", options.auto_clean || options.ephemeral);
        println!("keep_on_error={}", options.keep_on_error);
        println!("ephemeral={}", options.ephemeral);
        if needs_ephemeral_install {
            println!("ephemeral_install: {} install", tool);
        }
        print_env(ctx);
        return Ok(0);
    }

    let exit_code = run_command(ctx, tool, &final_args, cwd)?;

    if install_like && exit_code == 0 {
        sync_mutated_manifests_back(ctx)?;
    }

    if options.should_clean_after(exit_code) {
        clean_after_command(ctx);
    }

    Ok(exit_code)
}

fn run_install_like(ctx: &RimContext, tool: &str, dry_run: bool) -> Result<u8, String> {
    warn_about_low_rim_space(ctx);
    sync_manifests_to_shadow(ctx)?;
    let args = final_args(ctx, tool, vec![OsString::from("install")]);

    if dry_run {
        return Ok(0);
    }

    let code = run_command(ctx, tool, &args, &ctx.shadow_project)?;
    if code == 0 {
        sync_mutated_manifests_back(ctx)?;
    }
    Ok(code)
}

fn run_command(ctx: &RimContext, tool: &str, args: &[OsString], cwd: &Path) -> Result<u8, String> {
    let _signals = SignalGuard::install();
    let mut command = Command::new(tool);
    command
        .args(args)
        .current_dir(cwd)
        .env("npm_config_cache", &ctx.npm_cache)
        .env("XDG_CACHE_HOME", &ctx.xdg_cache)
        .env("TMPDIR", &ctx.tmp)
        .env("DENO_DIR", &ctx.deno_dir)
        .env("PLAYWRIGHT_BROWSERS_PATH", &ctx.playwright_browsers)
        .env("BUN_INSTALL_CACHE_DIR", &ctx.bun_cache);

    unsafe {
        command.pre_exec(|| {
            restore_default_signals();
            Ok(())
        });
    }

    let status = command
        .status()
        .map_err(|e| format!("failed to execute {tool}: {e}"))?;
    Ok(exit_status_code(status))
}

fn exit_status_code(status: ExitStatus) -> u8 {
    status
        .code()
        .map(|code| code.clamp(0, 255) as u8)
        .or_else(|| {
            status
                .signal()
                .map(|signal| (128 + signal).clamp(0, 255) as u8)
        })
        .unwrap_or(1)
}

fn clean_after_command(ctx: &RimContext) {
    if let Err(err) = clean(ctx) {
        eprintln!("rim: warning: auto-clean failed: {err}");
    }
}

fn should_ephemeral_install(tool: &str, args: &[OsString]) -> bool {
    if !matches!(tool, "npm" | "pnpm" | "bun" | "yarn") {
        return false;
    }
    let Some(first) = args.first().and_then(|arg| arg.to_str()) else {
        return false;
    };
    matches!(first, "run" | "test" | "start")
}

fn dependencies_missing(ctx: &RimContext) -> bool {
    if !ctx.node_modules.exists() {
        return true;
    }
    let Ok(entries) = fs::read_dir(&ctx.node_modules) else {
        return true;
    };
    for entry in entries.flatten() {
        if entry.file_name() != ".rim-keep" {
            return false;
        }
    }
    true
}

fn print_env(ctx: &RimContext) {
    println!("npm_config_cache={}", ctx.npm_cache.display());
    println!("XDG_CACHE_HOME={}", ctx.xdg_cache.display());
    println!("TMPDIR={}", ctx.tmp.display());
    println!("DENO_DIR={}", ctx.deno_dir.display());
    println!(
        "PLAYWRIGHT_BROWSERS_PATH={}",
        ctx.playwright_browsers.display()
    );
    println!("BUN_INSTALL_CACHE_DIR={}", ctx.bun_cache.display());
}

static SIGNAL_SEEN: AtomicBool = AtomicBool::new(false);
const SIGINT: i32 = 2;
const SIGTERM: i32 = 15;
const SIG_DFL: usize = 0;

type SignalHandler = usize;

unsafe extern "C" {
    fn signal(signum: i32, handler: SignalHandler) -> SignalHandler;
}

extern "C" fn record_signal(_signal: i32) {
    SIGNAL_SEEN.store(true, Ordering::SeqCst);
}

struct SignalGuard {
    previous_int: SignalHandler,
    previous_term: SignalHandler,
}

impl SignalGuard {
    fn install() -> Self {
        SIGNAL_SEEN.store(false, Ordering::SeqCst);
        let previous_int = unsafe { signal(SIGINT, record_signal as *const () as SignalHandler) };
        let previous_term = unsafe { signal(SIGTERM, record_signal as *const () as SignalHandler) };
        Self {
            previous_int,
            previous_term,
        }
    }
}

impl Drop for SignalGuard {
    fn drop(&mut self) {
        unsafe {
            signal(SIGINT, self.previous_int);
            signal(SIGTERM, self.previous_term);
        }
    }
}

fn restore_default_signals() {
    unsafe {
        signal(SIGINT, SIG_DFL);
        signal(SIGTERM, SIG_DFL);
    }
}

fn is_install_like(tool: &str, args: &[OsString]) -> bool {
    if !matches!(tool, "npm" | "pnpm" | "bun" | "yarn") {
        return false;
    }
    let Some(first) = args.first().and_then(|a| a.to_str()) else {
        return matches!(tool, "npm" | "pnpm" | "bun" | "yarn");
    };
    matches!(
        first,
        "install" | "i" | "add" | "remove" | "rm" | "update" | "up" | "ci"
    )
}

fn final_args(ctx: &RimContext, tool: &str, args: Vec<OsString>) -> Vec<OsString> {
    if tool == "pnpm" {
        let mut with_store = Vec::with_capacity(args.len() + 2);
        with_store.push(OsString::from("--store-dir"));
        with_store.push(ctx.pnpm_store.as_os_str().to_owned());
        with_store.extend(args);
        with_store
    } else {
        args
    }
}

fn join_args(args: &[OsString]) -> String {
    args.iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

fn print_context(ctx: &RimContext) {
    println!("project: {}", ctx.project_root.display());
    println!("rim_base: {}", ctx.rim_base.display());
    println!("rim_dir: {}", ctx.rim_dir.display());
    println!("shadow_project: {}", ctx.shadow_project.display());
    println!(
        "link: {} -> {}",
        ctx.project_root.join("node_modules").display(),
        ctx.node_modules.display()
    );
}
