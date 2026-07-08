mod platform;

use std::collections::BTreeMap;
use std::collections::hash_map::DefaultHasher;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{SystemTime, UNIX_EPOCH};

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
    keep_cache: bool,
    ensure_before_run: bool,
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

    platform::ensure_runtime_supported()?;

    let ctx = build_context()?;
    let mut command = command;
    let mut options = options;
    if is_manager_shortcut(&command) {
        let shortcut = command.clone();
        let manager = detect_manager(&ctx)?;
        if matches!(shortcut.as_str(), "run" | "test" | "start") {
            options.ensure_before_run = true;
        }
        let mut resolved = Vec::with_capacity(args.len() + 1);
        resolved.push(OsString::from(manager));
        resolved.extend(args);
        args = resolved;
        command = manager.to_owned();
    }

    match command.as_str() {
        "prepare" => {
            ensure_layout(&ctx)?;
            sync_manifests_to_shadow(&ctx)?;
            write_meta(&ctx, "prepare", true)?;
            print_context(&ctx);
            Ok(0)
        }
        "clean" => {
            let clean_args = args.split_off(1);
            clean_command(&ctx, &clean_args)
        }
        "scan" => {
            let scan_args = args.split_off(1);
            scan_command(&ctx, &scan_args)
        }
        "adopt" => {
            let adopt_args = args.split_off(1);
            adopt_command(&ctx, &adopt_args, options)
        }
        "backup" => {
            let backup_args = args.split_off(1);
            backup_command(&ctx, &backup_args)
        }
        "repair" => {
            let repair_args = args.split_off(1);
            repair_command(&ctx, &repair_args)
        }
        "ensure" => {
            let ensure_args = args.split_off(1);
            ensure_command(&ctx, &ensure_args, options)
        }
        "pin" => pin_command(&ctx, true),
        "unpin" => pin_command(&ctx, false),
        "manager" => manager_command(&ctx),
        "ls" => {
            list_layers(&ctx);
            Ok(0)
        }
        "gc" => {
            let gc_args = args.split_off(1);
            gc(&ctx, &gc_args)
        }
        "status" => {
            status(&ctx);
            Ok(0)
        }
        "path" => {
            let path_args = args.split_off(1);
            path_command(&ctx, &path_args)
        }
        "explain" => {
            let explain_args = args.split_off(1);
            explain(&ctx, &explain_args, options)
        }
        "doctor" => {
            let doctor_args = args.split_off(1);
            doctor_command(&ctx, &doctor_args)
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
            "--keep-cache" => options.keep_cache = true,
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
        "rim - dependency layer wrapper

Usage:
  rim prepare
  rim status
  rim doctor [--suggest]
  rim clean [--cache-only|--deps-only] [--force]
  rim scan [--json|--diff] <path>...
  rim adopt <project> [--dry-run] [--allow-risk] [--diff-backup] [--copy]
  rim backup list|show|restore <id|latest> [--dry-run] [--apply-deletes]
  rim repair --stale-locks|--broken-links [--dry-run]
  rim ensure [bun|npm|pnpm]
  rim pin|unpin
  rim manager
  rim ls
  rim gc [--dry-run] [--orphaned] [--older-than 1d] [--max-size 2g] [--all] [--include-pinned] [--force]
  rim path [--node-modules|--cache|--npm-cache|--bun-cache|--deno-cache|--tmp|--shadow]
  rim install|run|test|start|add|remove|update|ci [args...]  # auto-detect manager
  rim explain <bun|npm|deno|...> [args...]
  rim [--dry-run] [--auto-clean] [--ephemeral] [--keep-on-error] [--keep-cache] <bun|npm|deno|node|...> [args...]

Options:
  --dry-run        Show command/env without executing
  --auto-clean     Clean dependency layer after command exits
  --ephemeral      Fresh one-shot mode; implies --auto-clean
  --keep-on-error  Preserve layer when wrapped command fails
  --keep-cache     Keep npm/bun package-manager cache after successful installs

Environment:
  RIM_BASE      dependency layer base directory override
  RIM_PROFILE   ram|cache|external preset when RIM_BASE is unset"
    );
}

fn resolve_rim_base() -> Result<PathBuf, String> {
    if let Some(base) = env::var_os("RIM_BASE") {
        return Ok(PathBuf::from(base));
    }
    platform::default_rim_base()
}

fn build_context() -> Result<RimContext, String> {
    let cwd = env::current_dir().map_err(|e| format!("cannot read cwd: {e}"))?;
    let project_root = find_project_root(&cwd);
    let base = resolve_rim_base()?;
    Ok(build_context_for_project(project_root, base))
}

fn build_context_for_project(project_root: PathBuf, base: PathBuf) -> RimContext {
    let name = project_root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("project");
    let hash = short_hash(&project_root);
    let rim_dir = base.join(format!("{name}-{hash}"));
    let shadow_project = rim_dir.join("project");

    RimContext {
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
    }
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
    ensure_node_layout(ctx)
}

fn ensure_layout_for(ctx: &RimContext, tool: &str) -> Result<(), String> {
    if tool == "deno" {
        fs::create_dir_all(&ctx.rim_dir)
            .map_err(|e| format!("cannot create {}: {e}", ctx.rim_dir.display()))?;
        return Ok(());
    }
    ensure_node_layout(ctx)
}

fn ensure_node_layout(ctx: &RimContext) -> Result<(), String> {
    // Keep the base layout minimal. Tool-specific cache directories are created
    // lazily by the wrapped package manager only when it actually uses them.
    for dir in [&ctx.rim_dir, &ctx.shadow_project, &ctx.node_modules] {
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
        "bunfig.toml",
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
        "bunfig.toml",
        "yarn.lock",
    ]
}

fn ensure_node_modules_link(ctx: &RimContext) -> Result<(), String> {
    let link = ctx.project_root.join("node_modules");

    if platform::is_dir_link(&link) {
        let target = platform::read_dir_link(&link)
            .map_err(|e| format!("cannot read node_modules link: {e}"))?;
        if target != ctx.node_modules {
            platform::remove_dir_link(&link)
                .map_err(|e| format!("cannot replace node_modules link: {e}"))?;
            create_node_modules_link(&ctx.node_modules, &link)?;
        }
        return Ok(());
    }

    match fs::symlink_metadata(&link) {
        Ok(_) => Err(
            "node_modules exists and is not a directory link. Try: mv node_modules node_modules.backup && rim install"
                .to_owned(),
        ),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            create_node_modules_link(&ctx.node_modules, &link)
        }
        Err(e) => Err(format!("cannot inspect node_modules: {e}")),
    }
}

fn create_node_modules_link(target: &Path, link: &Path) -> Result<(), String> {
    platform::create_dir_link(target, link).map_err(|e| {
        if platform::platform_name() == "windows" {
            format!(
                "failed to create Windows directory symlink.\n\nWindows directory symlinks may require Developer Mode, SeCreateSymbolicLinkPrivilege,\nor running the terminal as Administrator.\n\nTry one of:\n  - enable Developer Mode\n  - run your terminal as Administrator\n  - use WSL\n  - set RIM_BASE to a persistent cache path and retry\n\nsource error: {e}"
            )
        } else {
            format!("cannot create node_modules symlink: {e}")
        }
    })
}

fn clean_command(ctx: &RimContext, args: &[OsString]) -> Result<u8, String> {
    let mut cache_only = false;
    let mut deps_only = false;
    let mut force = false;
    for arg in args {
        match arg.to_str() {
            Some("--cache-only") => cache_only = true,
            Some("--deps-only") => deps_only = true,
            Some("--force") => force = true,
            Some(other) => return Err(format!("unknown clean option: {other}")),
            None => return Err("clean options must be valid UTF-8".to_owned()),
        }
    }
    if cache_only && deps_only {
        return Err("--cache-only and --deps-only cannot be used together".to_owned());
    }
    guard_not_active(ctx, force)?;
    if cache_only {
        clean_cache(ctx)?;
        println!("cleaned cache: {}", ctx.rim_dir.display());
    } else if deps_only {
        clean_deps(ctx)?;
        println!("cleaned deps: {}", ctx.node_modules.display());
    } else {
        clean(ctx)?;
        println!("cleaned: {}", ctx.rim_dir.display());
    }
    Ok(0)
}

fn clean(ctx: &RimContext) -> Result<(), String> {
    clean_node_modules_link(ctx)?;
    if ctx.rim_dir.exists() {
        fs::remove_dir_all(&ctx.rim_dir)
            .map_err(|e| format!("cannot remove {}: {e}", ctx.rim_dir.display()))?;
    }
    Ok(())
}

fn clean_deps(ctx: &RimContext) -> Result<(), String> {
    clean_node_modules_link(ctx)?;
    if ctx.node_modules.exists() {
        fs::remove_dir_all(&ctx.node_modules)
            .map_err(|e| format!("cannot remove {}: {e}", ctx.node_modules.display()))?;
    }
    Ok(())
}

fn clean_cache(ctx: &RimContext) -> Result<(), String> {
    for path in [
        &ctx.npm_cache,
        &ctx.xdg_cache,
        &ctx.tmp,
        &ctx.deno_dir,
        &ctx.playwright_browsers,
        &ctx.bun_cache,
        &ctx.pnpm_store,
    ] {
        if path.exists() {
            fs::remove_dir_all(path)
                .map_err(|e| format!("cannot remove cache {}: {e}", path.display()))?;
        }
    }
    Ok(())
}

fn clean_node_modules_link(ctx: &RimContext) -> Result<(), String> {
    let link = ctx.project_root.join("node_modules");
    if platform::is_dir_link(&link) {
        let target = platform::read_dir_link(&link)
            .map_err(|e| format!("cannot read node_modules link: {e}"))?;
        if target == ctx.node_modules {
            platform::remove_dir_link(&link)
                .map_err(|e| format!("cannot remove node_modules link: {e}"))?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveState {
    None,
    Active(u32),
    Stale(u32),
}

impl ActiveState {
    fn is_active(self) -> bool {
        matches!(self, Self::Active(_))
    }

    fn label(self) -> String {
        match self {
            Self::None => "no".to_owned(),
            Self::Active(pid) => format!("pid:{pid}"),
            Self::Stale(pid) => format!("stale:{pid}"),
        }
    }
}

fn active_lock_path(rim_dir: &Path) -> PathBuf {
    rim_dir.join(".rim-active")
}

fn active_state(rim_dir: &Path) -> ActiveState {
    let path = active_lock_path(rim_dir);
    let Ok(contents) = fs::read_to_string(path) else {
        return ActiveState::None;
    };
    let Some(pid) = json_u64_field(&contents, "pid").and_then(|value| u32::try_from(value).ok())
    else {
        return ActiveState::Stale(0);
    };
    if platform::pid_is_alive(pid) {
        ActiveState::Active(pid)
    } else {
        ActiveState::Stale(pid)
    }
}

fn write_active_lock(ctx: &RimContext, tool: &str, args: &[OsString]) -> Result<(), String> {
    fs::create_dir_all(&ctx.rim_dir)
        .map_err(|e| format!("cannot create {}: {e}", ctx.rim_dir.display()))?;
    let command = format!("{} {}", tool, join_args(args));
    let contents = format!(
        "{{\n  \"pid\": {},\n  \"started_at\": {},\n  \"command\": \"{}\"\n}}\n",
        std::process::id(),
        now_unix(),
        json_escape(&command)
    );
    fs::write(active_lock_path(&ctx.rim_dir), contents)
        .map_err(|e| format!("cannot write active lock: {e}"))
}

fn remove_active_lock(ctx: &RimContext) {
    let _ = fs::remove_file(active_lock_path(&ctx.rim_dir));
}

fn guard_not_active(ctx: &RimContext, force: bool) -> Result<(), String> {
    match active_state(&ctx.rim_dir) {
        ActiveState::Active(pid) if !force => Err(format!(
            "rim layer is active by pid {pid}; pass --force only if you are sure"
        )),
        _ => Ok(()),
    }
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
struct RimMeta {
    project_root: String,
    created_at: u64,
    last_used_at: u64,
    manager: String,
    mode: String,
    rim_version: String,
    manifest_hash: Option<String>,
    pinned: bool,
}

#[derive(Debug, Clone)]
struct LayerInfo {
    rim_dir: PathBuf,
    meta: Option<RimMeta>,
    size_bytes: u64,
    active: ActiveState,
}

#[derive(Debug, Clone, Default)]
struct GcOptions {
    dry_run: bool,
    all: bool,
    orphaned: bool,
    older_than_seconds: Option<u64>,
    include_pinned: bool,
    force: bool,
    max_size_bytes: Option<u64>,
}

fn write_meta(ctx: &RimContext, manager: &str, update_manifest_hash: bool) -> Result<(), String> {
    write_meta_with_pin(ctx, manager, update_manifest_hash, None)
}

fn write_meta_with_pin(
    ctx: &RimContext,
    manager: &str,
    update_manifest_hash: bool,
    pinned_override: Option<bool>,
) -> Result<(), String> {
    let now = now_unix();
    let existing = read_meta(&ctx.rim_dir);
    let created_at = existing.as_ref().map_or(now, |meta| meta.created_at);
    let pinned =
        pinned_override.unwrap_or_else(|| existing.as_ref().is_some_and(|meta| meta.pinned));
    let manifest_hash = if update_manifest_hash {
        Some(manifest_hash(ctx))
    } else {
        existing
            .as_ref()
            .and_then(|meta| meta.manifest_hash.clone())
    };
    let manifest_hash_json = manifest_hash
        .as_ref()
        .map(|hash| format!("\"{}\"", json_escape(hash)))
        .unwrap_or_else(|| "null".to_owned());
    let contents = format!(
        "{{\n  \"schema_version\": 1,\n  \"project_root\": \"{}\",\n  \"created_at\": {},\n  \"last_used_at\": {},\n  \"manager\": \"{}\",\n  \"mode\": \"{}\",\n  \"rim_version\": \"{}\",\n  \"manifest_hash\": {},\n  \"pinned\": {}\n}}\n",
        json_escape(&ctx.project_root.to_string_lossy()),
        created_at,
        now,
        json_escape(manager),
        json_escape(&rim_mode(ctx)),
        json_escape(env!("CARGO_PKG_VERSION")),
        manifest_hash_json,
        pinned
    );
    let meta_path = ctx.rim_dir.join(".rim-meta.json");
    let tmp_path = ctx.rim_dir.join(".rim-meta.json.tmp");
    fs::write(&tmp_path, contents).map_err(|e| format!("cannot write rim metadata: {e}"))?;
    fs::rename(&tmp_path, &meta_path).map_err(|e| format!("cannot replace rim metadata: {e}"))
}

fn read_meta(rim_dir: &Path) -> Option<RimMeta> {
    let contents = fs::read_to_string(rim_dir.join(".rim-meta.json")).ok()?;
    Some(RimMeta {
        project_root: json_string_field(&contents, "project_root")?,
        created_at: json_u64_field(&contents, "created_at")?,
        last_used_at: json_u64_field(&contents, "last_used_at")?,
        manager: json_string_field(&contents, "manager").unwrap_or_else(|| "unknown".to_owned()),
        mode: json_string_field(&contents, "mode").unwrap_or_else(|| "unknown".to_owned()),
        rim_version: json_string_field(&contents, "rim_version")
            .unwrap_or_else(|| "unknown".to_owned()),
        manifest_hash: json_string_field(&contents, "manifest_hash"),
        pinned: json_bool_field(&contents, "pinned").unwrap_or(false),
    })
}

fn list_layers(ctx: &RimContext) {
    let layers = collect_layers(ctx);
    println!(
        "{:<36} {:<10} {:<8} {:>10} {:>8} {:>10} {:<6} {:<7} {:<8}  LAYER",
        "PROJECT", "MANAGER", "MODE", "SIZE", "AGE", "LAST_USED", "PIN", "ACTIVE", "VERSION"
    );
    for layer in layers {
        let project = layer
            .meta
            .as_ref()
            .map(|meta| shorten_project(&meta.project_root))
            .unwrap_or_else(|| "(unknown)".to_owned());
        let manager = layer
            .meta
            .as_ref()
            .map(|meta| meta.manager.as_str())
            .unwrap_or("unknown");
        let mode = layer
            .meta
            .as_ref()
            .map(|meta| meta.mode.as_str())
            .unwrap_or("unknown");
        let now = now_unix();
        let age = layer
            .meta
            .as_ref()
            .map(|meta| format_age(now.saturating_sub(meta.created_at)))
            .unwrap_or_else(|| "unknown".to_owned());
        let last_used = layer
            .meta
            .as_ref()
            .map(|meta| format_age(now.saturating_sub(meta.last_used_at)))
            .unwrap_or_else(|| "unknown".to_owned());
        let version = layer
            .meta
            .as_ref()
            .map(|meta| meta.rim_version.as_str())
            .unwrap_or("unknown");
        let pin = if layer.meta.as_ref().is_some_and(|meta| meta.pinned) {
            "yes"
        } else {
            "no"
        };
        let active = layer.active.label();
        println!(
            "{project:<36} {manager:<10} {mode:<8} {:>10} {:>8} {:>10} {pin:<6} {active:<7} {version:<8}  {}",
            format_bytes(layer.size_bytes),
            age,
            last_used,
            layer.rim_dir.display()
        );
    }
}

fn gc(ctx: &RimContext, args: &[OsString]) -> Result<u8, String> {
    let mut options = parse_gc_options(args)?;
    if !options.all
        && !options.orphaned
        && options.older_than_seconds.is_none()
        && options.max_size_bytes.is_none()
    {
        options.dry_run = true;
        options.orphaned = true;
        println!("rim gc: defaulting to --dry-run --orphaned");
    }

    if options.max_size_bytes.is_some() {
        return gc_max_size(ctx, &options);
    }

    let now = now_unix();
    let mut matched = 0_u64;
    let mut bytes = 0_u64;
    for layer in collect_layers(ctx) {
        if !gc_matches(&layer, &options, now) {
            continue;
        }
        if !options.force && layer.active.is_active() {
            println!("skipping active layer: {}", layer.rim_dir.display());
            continue;
        }
        matched += 1;
        bytes = bytes.saturating_add(layer.size_bytes);
        if options.dry_run {
            println!(
                "would remove {}  {}",
                format_bytes(layer.size_bytes),
                layer.rim_dir.display()
            );
        } else {
            println!(
                "removing {}  {}",
                format_bytes(layer.size_bytes),
                layer.rim_dir.display()
            );
            remove_layer(ctx, &layer.rim_dir)?;
        }
    }
    let action = if options.dry_run {
        "would remove"
    } else {
        "removed"
    };
    println!(
        "rim gc: {action} {matched} layer(s), {}",
        format_bytes(bytes)
    );
    Ok(0)
}

fn parse_gc_options(args: &[OsString]) -> Result<GcOptions, String> {
    let mut options = GcOptions::default();
    let mut i = 0;
    while i < args.len() {
        let Some(arg) = args[i].to_str() else {
            return Err("gc options must be valid UTF-8".to_owned());
        };
        match arg {
            "--dry-run" => options.dry_run = true,
            "--all" => options.all = true,
            "--orphaned" => options.orphaned = true,
            "--include-pinned" => options.include_pinned = true,
            "--force" => options.force = true,
            "--older-than" => {
                i += 1;
                let Some(value) = args.get(i).and_then(|arg| arg.to_str()) else {
                    return Err("--older-than requires a value like 1d, 6h, or 30m".to_owned());
                };
                options.older_than_seconds = Some(parse_duration_seconds(value)?);
            }
            "--max-size" => {
                i += 1;
                let Some(value) = args.get(i).and_then(|arg| arg.to_str()) else {
                    return Err("--max-size requires a value like 512m, 2g, or 100mb".to_owned());
                };
                options.max_size_bytes = Some(parse_size_bytes(value)?);
            }
            _ => return Err(format!("unknown gc option: {arg}")),
        }
        i += 1;
    }
    Ok(options)
}

fn parse_duration_seconds(value: &str) -> Result<u64, String> {
    let (number, unit) = value.split_at(
        value
            .trim_end_matches(|c: char| c.is_ascii_alphabetic())
            .len(),
    );
    let amount = number
        .parse::<u64>()
        .map_err(|_| format!("invalid duration: {value}"))?;
    match unit {
        "" | "s" => Ok(amount),
        "m" => Ok(amount.saturating_mul(60)),
        "h" => Ok(amount.saturating_mul(60 * 60)),
        "d" => Ok(amount.saturating_mul(60 * 60 * 24)),
        _ => Err(format!(
            "unsupported duration unit in {value}; use s, m, h, or d"
        )),
    }
}

fn parse_size_bytes(value: &str) -> Result<u64, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("invalid size: empty".to_owned());
    }
    let number_len = trimmed
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .map(char::len_utf8)
        .sum::<usize>();
    if number_len == 0 {
        return Err(format!("invalid size: {value}"));
    }
    let amount = trimmed[..number_len]
        .parse::<u64>()
        .map_err(|_| format!("invalid size: {value}"))?;
    let unit = trimmed[number_len..].trim().to_ascii_lowercase();
    let multiplier = match unit.as_str() {
        "" | "b" => 1,
        "k" | "kb" | "kib" => 1024,
        "m" | "mb" | "mib" => 1024_u64.saturating_pow(2),
        "g" | "gb" | "gib" => 1024_u64.saturating_pow(3),
        "t" | "tb" | "tib" => 1024_u64.saturating_pow(4),
        _ => {
            return Err(format!(
                "unsupported size unit in {value}; use b, k, m, g, or t"
            ));
        }
    };
    Ok(amount.saturating_mul(multiplier))
}

fn gc_max_size(ctx: &RimContext, options: &GcOptions) -> Result<u8, String> {
    let max_size = options.max_size_bytes.unwrap_or(0);
    let now = now_unix();
    let layers = collect_layers(ctx);
    let total_before = layers
        .iter()
        .fold(0_u64, |sum, layer| sum.saturating_add(layer.size_bytes));
    println!(
        "rim gc: max-size {}, current {}",
        format_bytes(max_size),
        format_bytes(total_before)
    );
    if total_before <= max_size {
        println!("rim gc: already within max-size budget");
        return Ok(0);
    }

    let mut projected = total_before;
    let mut matched = 0_u64;
    let mut bytes = 0_u64;
    for layer in layers {
        if projected <= max_size {
            break;
        }
        if !gc_budget_candidate(&layer, options, now) {
            continue;
        }
        if layer.rim_dir == ctx.rim_dir && !options.force {
            println!("skipping current layer: {}", layer.rim_dir.display());
            continue;
        }
        if !options.force && layer.active.is_active() {
            println!("skipping active layer: {}", layer.rim_dir.display());
            continue;
        }
        matched += 1;
        bytes = bytes.saturating_add(layer.size_bytes);
        projected = projected.saturating_sub(layer.size_bytes);
        if options.dry_run {
            println!(
                "would remove {}  {}",
                format_bytes(layer.size_bytes),
                layer.rim_dir.display()
            );
        } else {
            println!(
                "removing {}  {}",
                format_bytes(layer.size_bytes),
                layer.rim_dir.display()
            );
            remove_layer(ctx, &layer.rim_dir)?;
        }
    }
    let action = if options.dry_run {
        "would remove"
    } else {
        "removed"
    };
    println!(
        "rim gc: {action} {matched} layer(s), {}; projected total {}",
        format_bytes(bytes),
        format_bytes(projected)
    );
    if projected > max_size {
        println!(
            "rim gc: still above max-size by {}; pinned, active, current, or filtered layers may be protecting data",
            format_bytes(projected.saturating_sub(max_size))
        );
    }
    Ok(0)
}

fn gc_budget_candidate(layer: &LayerInfo, options: &GcOptions, now: u64) -> bool {
    if !options.include_pinned && layer.meta.as_ref().is_some_and(|meta| meta.pinned) {
        return false;
    }
    if options.all || (!options.orphaned && options.older_than_seconds.is_none()) {
        return true;
    }
    if options.orphaned
        && layer
            .meta
            .as_ref()
            .is_some_and(|meta| !Path::new(&meta.project_root).exists())
    {
        return true;
    }
    if let Some(older_than) = options.older_than_seconds
        && layer
            .meta
            .as_ref()
            .is_some_and(|meta| now.saturating_sub(meta.last_used_at) >= older_than)
    {
        return true;
    }
    false
}

fn gc_matches(layer: &LayerInfo, options: &GcOptions, now: u64) -> bool {
    if !options.include_pinned && layer.meta.as_ref().is_some_and(|meta| meta.pinned) {
        return false;
    }
    if options.all {
        return true;
    }
    if options.orphaned
        && layer
            .meta
            .as_ref()
            .is_some_and(|meta| !Path::new(&meta.project_root).exists())
    {
        return true;
    }
    if let Some(older_than) = options.older_than_seconds
        && layer
            .meta
            .as_ref()
            .is_some_and(|meta| now.saturating_sub(meta.last_used_at) >= older_than)
    {
        return true;
    }
    false
}

fn remove_layer(ctx: &RimContext, rim_dir: &Path) -> Result<(), String> {
    if rim_dir == ctx.rim_dir {
        return clean(ctx);
    }
    fs::remove_dir_all(rim_dir).map_err(|e| format!("cannot remove {}: {e}", rim_dir.display()))
}

fn collect_layers(ctx: &RimContext) -> Vec<LayerInfo> {
    let Ok(entries) = fs::read_dir(&ctx.rim_base) else {
        return Vec::new();
    };
    let mut layers = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .map(|rim_dir| LayerInfo {
            size_bytes: dir_size(&rim_dir).unwrap_or(0),
            meta: read_meta(&rim_dir),
            active: active_state(&rim_dir),
            rim_dir,
        })
        .collect::<Vec<_>>();
    layers.sort_by_key(|layer| {
        layer
            .meta
            .as_ref()
            .map_or(u64::MAX, |meta| meta.last_used_at)
    });
    layers
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn json_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

fn json_string_field(contents: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let rest = contents.split_once(&needle)?.1;
    let rest = rest.split_once(':')?.1.trim_start();
    let mut chars = rest.strip_prefix('"')?.chars();
    let mut value = String::new();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => return Some(value),
            '\\' => match chars.next()? {
                'n' => value.push('\n'),
                'r' => value.push('\r'),
                't' => value.push('\t'),
                other => value.push(other),
            },
            other => value.push(other),
        }
    }
    None
}

fn json_u64_field(contents: &str, key: &str) -> Option<u64> {
    let needle = format!("\"{key}\"");
    let rest = contents.split_once(&needle)?.1;
    let rest = rest.split_once(':')?.1.trim_start();
    let digits = rest
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    digits.parse::<u64>().ok()
}

fn json_bool_field(contents: &str, key: &str) -> Option<bool> {
    let needle = format!("\"{key}\"");
    let rest = contents.split_once(&needle)?.1;
    let rest = rest.split_once(':')?.1.trim_start();
    if rest.starts_with("true") {
        Some(true)
    } else if rest.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

fn shorten_project(project: &str) -> String {
    if let Some(home) = env::var_os("HOME") {
        let home = home.to_string_lossy();
        if let Some(rest) = project.strip_prefix(home.as_ref()) {
            return format!("~{rest}");
        }
    }
    project.to_owned()
}

fn format_age(seconds: u64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 60 * 60 {
        format!("{}m", seconds / 60)
    } else if seconds < 60 * 60 * 24 {
        format!("{}h", seconds / (60 * 60))
    } else {
        format!("{}d", seconds / (60 * 60 * 24))
    }
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

fn repair_command(ctx: &RimContext, args: &[OsString]) -> Result<u8, String> {
    let mut stale_locks = false;
    let mut broken_links = false;
    let mut dry_run = false;
    for arg in args {
        match arg.to_str() {
            Some("--stale-locks") => stale_locks = true,
            Some("--broken-links") => broken_links = true,
            Some("--dry-run") => dry_run = true,
            Some(other) => return Err(format!("unknown repair option: {other}")),
            None => return Err("repair options must be valid UTF-8".to_owned()),
        }
    }
    if !stale_locks && !broken_links {
        return Err("rim repair currently requires --stale-locks or --broken-links".to_owned());
    }
    if stale_locks {
        repair_stale_locks(ctx, dry_run)?;
    }
    if broken_links {
        repair_broken_links(ctx, dry_run)?;
    }
    Ok(0)
}

fn repair_stale_locks(ctx: &RimContext, dry_run: bool) -> Result<(), String> {
    let mut repaired = 0_u64;
    for layer in collect_layers(ctx) {
        if matches!(layer.active, ActiveState::Stale(_)) {
            let lock = active_lock_path(&layer.rim_dir);
            if dry_run {
                println!("would remove stale lock: {}", lock.display());
            } else {
                fs::remove_file(&lock)
                    .map_err(|e| format!("cannot remove stale lock {}: {e}", lock.display()))?;
                println!("removed stale lock: {}", lock.display());
            }
            repaired += 1;
        }
    }
    println!(
        "rim repair: {} {repaired} stale lock(s)",
        if dry_run { "would remove" } else { "removed" }
    );
    Ok(())
}

fn repair_broken_links(ctx: &RimContext, dry_run: bool) -> Result<(), String> {
    let link = ctx.project_root.join("node_modules");
    let mut repaired = 0_u64;
    if platform::is_dir_link(&link) {
        let target = platform::read_dir_link(&link)
            .map_err(|e| format!("cannot read node_modules link: {e}"))?;
        if target.starts_with(&ctx.rim_base) && !target.exists() {
            if dry_run {
                println!(
                    "would remove broken node_modules link: {} -> {}",
                    link.display(),
                    target.display()
                );
            } else {
                platform::remove_dir_link(&link).map_err(|e| {
                    format!(
                        "cannot remove broken node_modules link {}: {e}",
                        link.display()
                    )
                })?;
                println!(
                    "removed broken node_modules link: {} -> {}",
                    link.display(),
                    target.display()
                );
            }
            repaired += 1;
        }
    }
    println!(
        "rim repair: {} {repaired} broken node_modules link(s)",
        if dry_run { "would remove" } else { "removed" }
    );
    if repaired > 0 {
        println!("next: rim ensure");
    }
    Ok(())
}

fn pin_command(ctx: &RimContext, pinned: bool) -> Result<u8, String> {
    fs::create_dir_all(&ctx.rim_dir)
        .map_err(|e| format!("cannot create {}: {e}", ctx.rim_dir.display()))?;
    let manager = read_meta(&ctx.rim_dir)
        .map(|meta| meta.manager)
        .unwrap_or_else(|| detect_manager(ctx).unwrap_or("unknown").to_owned());
    write_meta_with_pin(ctx, &manager, false, Some(pinned))?;
    println!(
        "{}: {}",
        if pinned { "pinned" } else { "unpinned" },
        ctx.rim_dir.display()
    );
    Ok(0)
}

fn manager_command(ctx: &RimContext) -> Result<u8, String> {
    let detection = detect_manager_with_reason(ctx)?;
    println!("detected: {}", detection.manager);
    println!("reason: {}", detection.reason);
    Ok(0)
}

#[derive(Debug, Clone)]
struct ScanCandidate {
    project_root: PathBuf,
    node_modules: PathBuf,
    size_bytes: u64,
    manager: String,
    risk: String,
    action: String,
    warnings: Vec<String>,
    managed: bool,
}

#[derive(Debug, Clone)]
struct TreeEntry {
    kind: String,
    size: u64,
    hash: Option<String>,
    link_target: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct DiffCounts {
    changed: u64,
    added: u64,
    deleted: u64,
    type_changed: u64,
    binary: u64,
}

fn scan_command(ctx: &RimContext, args: &[OsString]) -> Result<u8, String> {
    let mut json = false;
    let mut diff = false;
    let mut paths = Vec::new();
    for arg in args {
        match arg.to_str() {
            Some("--json") => json = true,
            Some("--diff") => diff = true,
            Some(path) => paths.push(expand_tilde(path)),
            None => return Err("scan arguments must be valid UTF-8".to_owned()),
        }
    }
    if paths.is_empty() {
        paths.push(env::current_dir().map_err(|e| format!("cannot read cwd: {e}"))?);
    }

    let mut candidates = Vec::new();
    for path in paths {
        scan_path(ctx, &path, &mut candidates)?;
    }

    if diff {
        let unmanaged = candidates
            .iter()
            .filter(|candidate| !candidate.managed)
            .collect::<Vec<_>>();
        if unmanaged.len() != 1 {
            return Err(format!(
                "rim scan --diff requires exactly one unmanaged candidate; found {}. Pass a specific project path.",
                unmanaged.len()
            ));
        }
        let candidate = unmanaged[0];
        let diff = compare_with_fresh_install(ctx, candidate, false, true)?;
        if json {
            print_scan_json(&candidates, Some(&diff));
        } else {
            print_scan_table(&candidates);
            println!(
                "manual_diff: {}",
                if diff.has_changes() {
                    "detected"
                } else {
                    "none"
                }
            );
            println!(
                "diff_summary: changed={} added={} deleted={} type_changed={} binary={}",
                diff.changed, diff.added, diff.deleted, diff.type_changed, diff.binary
            );
        }
        return Ok(0);
    }

    if json {
        print_scan_json(&candidates, None);
    } else {
        print_scan_table(&candidates);
    }
    Ok(0)
}

fn scan_path(ctx: &RimContext, root: &Path, out: &mut Vec<ScanCandidate>) -> Result<(), String> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == "node_modules" {
                out.push(analyze_node_modules(ctx, &path));
                continue;
            }
            if should_skip_scan_dir(&name) {
                continue;
            }
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.is_dir() && !meta.file_type().is_symlink() {
                stack.push(path);
            }
        }
    }
    Ok(())
}

fn should_skip_scan_dir(name: &str) -> bool {
    matches!(
        name,
        ".git" | ".rim-backups" | "target" | "dist" | "build" | ".next" | ".cache" | ".turbo"
    )
}

fn analyze_node_modules(ctx: &RimContext, node_modules: &Path) -> ScanCandidate {
    let project_root = node_modules
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let is_symlink = platform::is_dir_link(node_modules);
    let managed = is_symlink
        && platform::read_dir_link(node_modules)
            .map(|target| target.starts_with(&ctx.rim_base))
            .unwrap_or(false);
    let size_bytes = if is_symlink {
        0
    } else {
        dir_size(node_modules).unwrap_or(0)
    };
    let (manager, manager_warning) = infer_manager_for_project(&project_root, node_modules);
    let mut warnings = Vec::new();
    if let Some(warning) = manager_warning {
        warnings.push(warning);
    }
    if managed {
        warnings.push("already managed by rim".to_owned());
    }
    if is_symlink && !managed {
        warnings.push("node_modules is a symlink".to_owned());
    }
    if !project_root.join("package.json").exists() {
        warnings.push("package.json missing".to_owned());
    }
    if workspace_detected_in(&project_root) {
        warnings.push("workspace detected".to_owned());
    }
    if lifecycle_scripts_detected_in(&project_root) {
        warnings.push("lifecycle scripts detected".to_owned());
    }
    let heavy = risky_packages_in(&project_root);
    if !heavy.is_empty() {
        warnings.push(format!("heavy packages detected: {}", heavy.join(", ")));
    }
    if size_bytes > 512 * 1024 * 1024 {
        warnings.push("large node_modules".to_owned());
    }

    let high = managed
        || is_symlink
        || !project_root.join("package.json").exists()
        || manager == "pnpm"
        || workspace_detected_in(&project_root);
    let medium = manager == "unknown"
        || manager == "yarn"
        || !has_any_lockfile(&project_root)
        || lifecycle_scripts_detected_in(&project_root)
        || !heavy.is_empty()
        || size_bytes > 512 * 1024 * 1024;
    let risk = if high {
        "high"
    } else if medium {
        "medium"
    } else {
        "low"
    }
    .to_owned();
    let action = if managed || risk == "high" {
        "skip"
    } else if risk == "medium" {
        "review"
    } else {
        "adoptable"
    }
    .to_owned();

    ScanCandidate {
        project_root,
        node_modules: node_modules.to_path_buf(),
        size_bytes,
        manager,
        risk,
        action,
        warnings,
        managed,
    }
}

fn infer_manager_for_project(project_root: &Path, node_modules: &Path) -> (String, Option<String>) {
    let package_json = fs::read_to_string(project_root.join("package.json")).unwrap_or_default();
    if let Some(pm) = json_string_field(&package_json, "packageManager")
        && let Some((manager, _version)) = pm.split_once('@')
    {
        return (manager.to_owned(), Some(format!("packageManager {pm}")));
    }
    if project_root.join("bun.lock").exists() || project_root.join("bun.lockb").exists() {
        return ("bun".to_owned(), None);
    }
    if project_root.join("package-lock.json").exists()
        || project_root.join("npm-shrinkwrap.json").exists()
    {
        return ("npm".to_owned(), None);
    }
    if project_root.join("pnpm-lock.yaml").exists() || node_modules.join(".modules.yaml").exists() {
        return (
            "pnpm".to_owned(),
            Some("pnpm store layout is high-risk for adopt".to_owned()),
        );
    }
    if project_root.join("yarn.lock").exists() {
        return (
            "yarn".to_owned(),
            Some("yarn adopt is medium-risk".to_owned()),
        );
    }
    if project_root.join("package.json").exists() {
        return (
            "unknown".to_owned(),
            Some("package.json found but no lockfile".to_owned()),
        );
    }
    (
        "unknown".to_owned(),
        Some("project root has no package.json".to_owned()),
    )
}

fn print_scan_table(candidates: &[ScanCandidate]) {
    println!(
        "{:<36} {:>10} {:<8} {:<7} {:<10} WARNINGS",
        "PROJECT", "SIZE", "MANAGER", "RISK", "ACTION"
    );
    for candidate in candidates {
        println!(
            "{:<36} {:>10} {:<8} {:<7} {:<10} {}",
            shorten_project(&candidate.project_root.to_string_lossy()),
            format_bytes(candidate.size_bytes),
            candidate.manager,
            candidate.risk,
            candidate.action,
            candidate.warnings.join("; ")
        );
    }
}

fn print_scan_json(candidates: &[ScanCandidate], diff: Option<&DiffCounts>) {
    println!("{{");
    println!("  \"candidates\": [");
    for (idx, candidate) in candidates.iter().enumerate() {
        let comma = if idx + 1 == candidates.len() { "" } else { "," };
        println!(
            "    {{\"project\":\"{}\",\"node_modules\":\"{}\",\"size_bytes\":{},\"manager\":\"{}\",\"risk\":\"{}\",\"action\":\"{}\",\"managed\":{},\"warnings\":[{}]}}{}",
            json_escape(&candidate.project_root.to_string_lossy()),
            json_escape(&candidate.node_modules.to_string_lossy()),
            candidate.size_bytes,
            json_escape(&candidate.manager),
            candidate.risk,
            candidate.action,
            candidate.managed,
            candidate
                .warnings
                .iter()
                .map(|w| format!("\"{}\"", json_escape(w)))
                .collect::<Vec<_>>()
                .join(","),
            comma
        );
    }
    println!("  ]{}", if diff.is_some() { "," } else { "" });
    if let Some(diff) = diff {
        println!(
            "  \"manual_diff\": \"{}\",",
            if diff.has_changes() {
                "detected"
            } else {
                "none"
            }
        );
        println!("  \"diff\": {}", diff.to_json());
    }
    println!("}}");
}

fn adopt_command(
    base_ctx: &RimContext,
    args: &[OsString],
    global_options: CliOptions,
) -> Result<u8, String> {
    let mut dry_run = global_options.dry_run;
    let mut allow_risk = false;
    let mut diff_backup = false;
    let mut copy_mode = false;
    let mut project_arg: Option<PathBuf> = None;
    for arg in args {
        match arg.to_str() {
            Some("--dry-run") => dry_run = true,
            Some("--allow-risk") => allow_risk = true,
            Some("--diff-backup") => diff_backup = true,
            Some("--copy") => copy_mode = true,
            Some(path) if project_arg.is_none() => project_arg = Some(expand_tilde(path)),
            Some(other) => return Err(format!("unknown adopt option or extra path: {other}")),
            None => return Err("adopt arguments must be valid UTF-8".to_owned()),
        }
    }
    let Some(project_arg) = project_arg else {
        return Err("rim adopt requires a project path".to_owned());
    };
    let project_root = find_project_root(&project_arg);
    let ctx = build_context_for_project(project_root.clone(), base_ctx.rim_base.clone());
    let node_modules = project_root.join("node_modules");
    let candidate = analyze_node_modules(&ctx, &node_modules);
    print_adopt_plan(&ctx, &candidate, dry_run, diff_backup, copy_mode);

    if candidate.managed {
        return Err("node_modules is already managed by rim".to_owned());
    }
    if candidate.risk == "high" && !allow_risk {
        return Err(
            "high-risk adopt refused; pass --allow-risk after reviewing warnings".to_owned(),
        );
    }
    let meta = fs::symlink_metadata(&node_modules)
        .map_err(|_| format!("{} does not exist", node_modules.display()))?;
    if !meta.is_dir() || platform::is_dir_link(&node_modules) {
        return Err(
            "adopt requires a real node_modules directory, not a symlink or file".to_owned(),
        );
    }
    if dry_run {
        if diff_backup {
            println!("diff_backup: would compare existing node_modules with a fresh install");
            println!("diff_backup: dry-run does not create scratch files or backups");
        }
        if copy_mode {
            println!(
                "copy: would copy node_modules into the rim layer and move the original into .rim-backups"
            );
            println!("copy: dry-run does not create backups, layer files, or symlinks");
        }
        return Ok(0);
    }

    guard_not_active(&ctx, false)?;

    if diff_backup {
        create_diff_backup(&ctx, &candidate, allow_risk)?;
    }

    fs::create_dir_all(&ctx.shadow_project)
        .map_err(|e| format!("cannot create shadow project: {e}"))?;
    if ctx.node_modules.exists() {
        return Err(format!(
            "target layer already has node_modules: {}",
            ctx.node_modules.display()
        ));
    }
    if copy_mode {
        adopt_copy_mode(&ctx, &node_modules, &candidate.manager)
    } else {
        move_dir_cross_device(&node_modules, &ctx.node_modules)?;
        let result = finish_adopt_after_move(
            &ctx,
            &node_modules,
            &candidate.manager,
            "move-existing-node_modules",
        );
        match result {
            Ok(()) => {
                println!(
                    "adopted: {} -> {}",
                    node_modules.display(),
                    ctx.node_modules.display()
                );
                Ok(0)
            }
            Err(err) => match rollback_adopt_move(&ctx, &node_modules) {
                Ok(()) => Err(format!(
                    "{err}
rim: adopt failed after moving node_modules; rollback restored {}",
                    node_modules.display()
                )),
                Err(rollback_err) => Err(format!(
                    "{err}\nrim: adopt failed after moving node_modules.\nrim: rollback failed: {rollback_err}\nrim: your node_modules is still at:\n  {}\n\n{}",
                    ctx.node_modules.display(),
                    manual_recovery_message(&ctx.node_modules, &node_modules)
                )),
            },
        }
    }
}

fn adopt_copy_mode(
    ctx: &RimContext,
    project_node_modules: &Path,
    manager: &str,
) -> Result<u8, String> {
    eprintln!("rim: warning: --copy temporarily uses roughly 2x node_modules space.");
    copy_dir_recursive(project_node_modules, &ctx.node_modules)?;
    let original_backup = backup_root(ctx).join(format!("node_modules-original-{}", now_unix()));
    fs::create_dir_all(backup_root(ctx)).map_err(|e| format!("cannot create backup root: {e}"))?;
    if original_backup.exists() {
        return Err(format!(
            "backup path already exists: {}",
            original_backup.display()
        ));
    }
    move_dir_cross_device(project_node_modules, &original_backup)?;
    let result = finish_adopt_after_move(
        ctx,
        project_node_modules,
        manager,
        "copy-existing-node_modules",
    );
    match result {
        Ok(()) => {
            println!(
                "adopted(copy): {} -> {}",
                project_node_modules.display(),
                ctx.node_modules.display()
            );
            println!("original_backup: {}", original_backup.display());
            Ok(0)
        }
        Err(err) => match rollback_adopt_copy(ctx, project_node_modules, &original_backup) {
            Ok(()) => Err(format!(
                "{err}\nrim: adopt --copy failed; rollback restored {}",
                project_node_modules.display()
            )),
            Err(rollback_err) => Err(format!(
                "{err}\nrim: adopt --copy failed.\nrim: rollback failed: {rollback_err}\nrim: copied layer node_modules is at:\n  {}\nrim: original backup is at:\n  {}\n\nTo recover manually, move the original backup back or recreate the link.",
                ctx.node_modules.display(),
                original_backup.display()
            )),
        },
    }
}

fn rollback_adopt_copy(
    ctx: &RimContext,
    project_node_modules: &Path,
    original_backup: &Path,
) -> Result<(), String> {
    if platform::is_dir_link(project_node_modules) {
        platform::remove_dir_link(project_node_modules).map_err(|e| {
            format!(
                "cannot remove failed link {}: {e}",
                project_node_modules.display()
            )
        })?;
    }
    if project_node_modules.exists() {
        return Err(format!(
            "{} already exists, cannot restore original backup automatically",
            project_node_modules.display()
        ));
    }
    move_dir_cross_device(original_backup, project_node_modules)?;
    if ctx.node_modules.exists() {
        let _ = fs::remove_dir_all(&ctx.node_modules);
    }
    Ok(())
}

fn manual_recovery_message(layer_node_modules: &Path, project_node_modules: &Path) -> String {
    if platform::platform_name() == "windows" {
        format!(
            "To recover manually on Windows:\n  mklink /D {} {}\n\nor move it back:\n  move {} {}\n\nNote: mklink /D may require Developer Mode, SeCreateSymbolicLinkPrivilege, or Administrator privileges.",
            project_node_modules.display(),
            layer_node_modules.display(),
            layer_node_modules.display(),
            project_node_modules.display()
        )
    } else {
        format!(
            "To recover manually:\n  ln -s {} {}\nor:\n  mv {} {}",
            layer_node_modules.display(),
            project_node_modules.display(),
            layer_node_modules.display(),
            project_node_modules.display()
        )
    }
}

fn finish_adopt_after_move(
    ctx: &RimContext,
    project_node_modules: &Path,
    manager: &str,
    adopt_method: &str,
) -> Result<(), String> {
    if env::var_os("RIM_TEST_FAIL_ADOPT_SYMLINK").is_some() {
        return Err("cannot create node_modules symlink: simulated failure".to_owned());
    }
    create_node_modules_link(&ctx.node_modules, project_node_modules)?;
    if env::var_os("RIM_TEST_FAIL_ADOPT_META").is_some() {
        return Err("cannot write rim metadata: simulated failure".to_owned());
    }
    write_adopt_meta(ctx, manager, adopt_method)
}

fn rollback_adopt_move(ctx: &RimContext, project_node_modules: &Path) -> Result<(), String> {
    if env::var_os("RIM_TEST_BLOCK_ADOPT_ROLLBACK").is_some() {
        fs::create_dir_all(project_node_modules)
            .map_err(|e| format!("cannot create rollback blocker: {e}"))?;
    }
    if platform::is_dir_link(project_node_modules) {
        platform::remove_dir_link(project_node_modules).map_err(|e| {
            format!(
                "cannot remove failed link {}: {e}",
                project_node_modules.display()
            )
        })?;
    }
    if project_node_modules.exists() {
        return Err(format!(
            "{} already exists, cannot restore automatically",
            project_node_modules.display()
        ));
    }
    move_dir_cross_device(&ctx.node_modules, project_node_modules)
}

fn print_adopt_plan(
    ctx: &RimContext,
    candidate: &ScanCandidate,
    dry_run: bool,
    diff_backup: bool,
    copy_mode: bool,
) {
    println!("project: {}", candidate.project_root.display());
    println!("node_modules: {}", candidate.node_modules.display());
    println!("rim_dir: {}", ctx.rim_dir.display());
    println!("manager: {}", candidate.manager);
    println!("risk: {}", candidate.risk);
    println!("action: {}", candidate.action);
    if dry_run {
        println!("dry_run: true");
    }
    if diff_backup {
        println!("diff_backup: enabled");
    }
    if copy_mode {
        println!("copy: enabled");
    }
    for warning in &candidate.warnings {
        eprintln!("rim: warning: {warning}");
    }
    if rim_mode(ctx) == "tmpfs" {
        eprintln!(
            "rim: warning: adopting into tmpfs; adopted node_modules will disappear on reboot."
        );
        eprintln!(
            "rim: warning: use --diff-backup, RIM_PROFILE=cache, or RIM_PROFILE=external for hand-edited node_modules."
        );
    }
    eprintln!(
        "rim: warning: manual node_modules edits cannot be fully detected without --diff-backup."
    );
}

fn write_adopt_meta(ctx: &RimContext, manager: &str, adopt_method: &str) -> Result<(), String> {
    let now = now_unix();
    let contents = format!(
        "{{\n  \"schema_version\": 1,\n  \"project_root\": \"{}\",\n  \"created_at\": {},\n  \"last_used_at\": {},\n  \"manager\": \"{}\",\n  \"mode\": \"{}\",\n  \"rim_version\": \"{}\",\n  \"manifest_hash\": \"{}\",\n  \"pinned\": false,\n  \"adopted\": true,\n  \"adopted_at\": {},\n  \"adopt_method\": \"{}\"\n}}\n",
        json_escape(&ctx.project_root.to_string_lossy()),
        now,
        now,
        json_escape(manager),
        json_escape(&rim_mode(ctx)),
        json_escape(env!("CARGO_PKG_VERSION")),
        manifest_hash(ctx),
        now,
        json_escape(adopt_method)
    );
    fs::write(ctx.rim_dir.join(".rim-meta.json.tmp"), contents)
        .map_err(|e| format!("cannot write rim metadata: {e}"))?;
    fs::rename(
        ctx.rim_dir.join(".rim-meta.json.tmp"),
        ctx.rim_dir.join(".rim-meta.json"),
    )
    .map_err(|e| format!("cannot replace rim metadata: {e}"))
}

fn create_diff_backup(
    ctx: &RimContext,
    candidate: &ScanCandidate,
    allow_risk: bool,
) -> Result<Option<PathBuf>, String> {
    if candidate.manager == "pnpm" && !allow_risk {
        return Err("diff-backup for pnpm requires --allow-risk".to_owned());
    }
    if !matches!(candidate.manager.as_str(), "npm" | "bun" | "pnpm" | "yarn") {
        return Err("diff-backup requires npm, bun, pnpm, or yarn manager detection".to_owned());
    }
    let scratch = ctx.rim_dir.join(format!("adopt-scratch-{}", now_unix()));
    let result = create_diff_backup_inner(ctx, candidate, &scratch);
    cleanup_scratch(&scratch);
    result
}

fn create_diff_backup_inner(
    ctx: &RimContext,
    candidate: &ScanCandidate,
    scratch: &Path,
) -> Result<Option<PathBuf>, String> {
    if scratch.exists() {
        fs::remove_dir_all(scratch).map_err(|e| format!("cannot clear scratch: {e}"))?;
    }
    fs::create_dir_all(scratch).map_err(|e| format!("cannot create scratch: {e}"))?;
    copy_manifests_to_dir(&ctx.project_root, scratch)?;
    run_pristine_install(&candidate.manager, scratch, ctx)?;
    let pristine = scratch.join("node_modules");
    if !pristine.exists() {
        return Err("fresh install did not create scratch node_modules".to_owned());
    }
    let backup = backup_root(ctx).join(format!("node_modules-delta-{}", now_unix()));
    fs::create_dir_all(&backup).map_err(|e| format!("cannot create backup dir: {e}"))?;
    let diff = diff_trees(&candidate.node_modules, &pristine, &backup)?;
    write_backup_metadata(ctx, &backup, &candidate.manager, &diff)?;
    println!("diff_backup: {}", backup.display());
    Ok(Some(backup))
}

fn compare_with_fresh_install(
    ctx: &RimContext,
    candidate: &ScanCandidate,
    allow_risk: bool,
    cleanup: bool,
) -> Result<DiffCounts, String> {
    if candidate.manager == "pnpm" && !allow_risk {
        return Err(
            "scan --diff for pnpm requires --allow-risk via adopt; use a specific low-risk project"
                .to_owned(),
        );
    }
    let scratch = ctx
        .rim_dir
        .join(format!("scan-diff-scratch-{}", now_unix()));
    let result = compare_with_fresh_install_inner(ctx, candidate, &scratch);
    if cleanup {
        cleanup_scratch(&scratch);
    }
    result
}

fn compare_with_fresh_install_inner(
    ctx: &RimContext,
    candidate: &ScanCandidate,
    scratch: &Path,
) -> Result<DiffCounts, String> {
    fs::create_dir_all(scratch).map_err(|e| format!("cannot create scratch: {e}"))?;
    copy_manifests_to_dir(&candidate.project_root, scratch)?;
    run_pristine_install(&candidate.manager, scratch, ctx)?;
    let backup = scratch.join("diff-output");
    fs::create_dir_all(&backup).map_err(|e| format!("cannot create diff output: {e}"))?;
    diff_trees(
        &candidate.node_modules,
        &scratch.join("node_modules"),
        &backup,
    )
}

fn copy_manifests_to_dir(project_root: &Path, dst_dir: &Path) -> Result<(), String> {
    for name in manifest_names() {
        let src = project_root.join(name);
        if src.exists() {
            fs::copy(&src, dst_dir.join(name))
                .map_err(|e| format!("cannot copy manifest {} to scratch: {e}", src.display()))?;
        }
    }
    Ok(())
}

fn cleanup_scratch(scratch: &Path) {
    if scratch.exists()
        && let Err(err) = fs::remove_dir_all(scratch)
    {
        eprintln!(
            "rim: warning: failed to remove scratch {}: {err}",
            scratch.display()
        );
    }
}

fn run_pristine_install(manager: &str, cwd: &Path, ctx: &RimContext) -> Result<(), String> {
    let (tool, args): (&str, Vec<&str>) = match manager {
        "npm" => (
            "npm",
            vec!["install", "--ignore-scripts", "--no-audit", "--no-fund"],
        ),
        "bun" => ("bun", vec!["install", "--ignore-scripts"]),
        "pnpm" => ("pnpm", vec!["install", "--ignore-scripts"]),
        "yarn" => ("yarn", vec!["install", "--ignore-scripts"]),
        other => return Err(format!("unsupported manager for diff-backup: {other}")),
    };
    let status = Command::new(tool)
        .args(args)
        .current_dir(cwd)
        .env(
            "npm_config_cache",
            ctx.rim_dir.join("adopt-pristine-npm-cache"),
        )
        .env(
            "BUN_INSTALL_CACHE_DIR",
            ctx.rim_dir.join("adopt-pristine-bun-cache"),
        )
        .status()
        .map_err(|e| format!("failed to run pristine install with {tool}: {e}"))?;
    if !status.success() {
        return Err(format!("pristine install failed with {tool}"));
    }
    Ok(())
}

fn diff_trees(existing: &Path, pristine: &Path, backup: &Path) -> Result<DiffCounts, String> {
    if env::var_os("RIM_TEST_FAIL_DIFF").is_some() {
        return Err("simulated diff failure".to_owned());
    }
    let existing_map = snapshot_tree(existing)?;
    let pristine_map = snapshot_tree(pristine)?;
    let mut diff = DiffCounts::default();
    let mut deleted = Vec::new();
    let mut symlinks = Vec::new();

    for (rel, entry) in &existing_map {
        match pristine_map.get(rel) {
            None => {
                diff.added += 1;
                copy_backup_item(existing, rel, entry, &backup.join("added"))?;
            }
            Some(base) if entry.kind != base.kind => {
                diff.type_changed += 1;
                copy_backup_item(existing, rel, entry, &backup.join("changed"))?;
            }
            Some(base)
                if entry.kind != "dir"
                    && (entry.hash != base.hash
                        || entry.link_target != base.link_target
                        || entry.size != base.size) =>
            {
                if entry.kind == "symlink" {
                    symlinks.push(format!(
                        "{{\"path\":\"node_modules/{}\",\"target\":\"{}\"}}",
                        json_escape(rel),
                        json_escape(entry.link_target.as_deref().unwrap_or(""))
                    ));
                    diff.changed += 1;
                } else if is_binary_or_large(&existing.join(rel)) {
                    diff.binary += 1;
                    copy_backup_item(existing, rel, entry, &backup.join("binary"))?;
                } else {
                    diff.changed += 1;
                    copy_backup_item(existing, rel, entry, &backup.join("changed"))?;
                }
            }
            _ => {}
        }
    }
    for rel in pristine_map.keys() {
        if !existing_map.contains_key(rel) {
            diff.deleted += 1;
            deleted.push(format!("\"node_modules/{}\"", json_escape(rel)));
        }
    }
    fs::write(
        backup.join("deleted.json"),
        format!("[{}]\n", deleted.join(",")),
    )
    .map_err(|e| format!("cannot write deleted.json: {e}"))?;
    fs::write(
        backup.join("symlinks.json"),
        format!("[{}]\n", symlinks.join(",")),
    )
    .map_err(|e| format!("cannot write symlinks.json: {e}"))?;
    Ok(diff)
}

fn snapshot_tree(root: &Path) -> Result<BTreeMap<String, TreeEntry>, String> {
    let mut map = BTreeMap::new();
    if !root.exists() {
        return Ok(map);
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        for entry in
            fs::read_dir(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?
        {
            let entry = entry.map_err(|e| format!("cannot read dir entry: {e}"))?;
            let child = entry.path();
            let rel = child
                .strip_prefix(root)
                .unwrap_or(&child)
                .to_string_lossy()
                .to_string();
            let meta = fs::symlink_metadata(&child)
                .map_err(|e| format!("cannot stat {}: {e}", child.display()))?;
            let file_type = meta.file_type();
            if file_type.is_symlink() {
                map.insert(
                    rel,
                    TreeEntry {
                        kind: "symlink".to_owned(),
                        size: meta.len(),
                        hash: None,
                        link_target: platform::read_dir_link(&child)
                            .ok()
                            .map(|p| p.to_string_lossy().to_string()),
                    },
                );
            } else if meta.is_dir() {
                map.insert(
                    rel,
                    TreeEntry {
                        kind: "dir".to_owned(),
                        size: meta.len(),
                        hash: None,
                        link_target: None,
                    },
                );
                stack.push(child);
            } else if meta.is_file() {
                map.insert(
                    rel,
                    TreeEntry {
                        kind: "file".to_owned(),
                        size: meta.len(),
                        hash: Some(file_hash(&child)?),
                        link_target: None,
                    },
                );
            }
        }
    }
    Ok(map)
}

fn file_hash(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    Ok(format!("{:016x}", hasher.finish()))
}

fn copy_backup_item(
    root: &Path,
    rel: &str,
    entry: &TreeEntry,
    dst_root: &Path,
) -> Result<(), String> {
    if entry.kind == "dir" {
        return Ok(());
    }
    let src = root.join(rel);
    let dst = dst_root.join("node_modules").join(rel);
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create backup parent {}: {e}", parent.display()))?;
    }
    if entry.kind == "symlink" {
        if let Some(target) = &entry.link_target {
            platform::create_dir_link(Path::new(target), &dst)
                .map_err(|e| format!("cannot backup directory link {}: {e}", dst.display()))?;
        }
    } else {
        fs::copy(&src, &dst)
            .map_err(|e| format!("cannot copy backup item {}: {e}", src.display()))?;
    }
    Ok(())
}

fn is_binary_or_large(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    if meta.len() > 1024 * 1024 {
        return true;
    }
    let Ok(bytes) = fs::read(path) else {
        return true;
    };
    std::str::from_utf8(&bytes).is_err()
}

fn write_backup_metadata(
    ctx: &RimContext,
    backup: &Path,
    manager: &str,
    diff: &DiffCounts,
) -> Result<(), String> {
    let metadata = format!(
        "{{\n  \"schema_version\": 1,\n  \"project_root\": \"{}\",\n  \"manager\": \"{}\",\n  \"created_at\": {},\n  \"baseline_method\": \"fresh-install\",\n  \"manifest_hash\": \"{}\",\n  \"differences\": {{\"changed\": {}, \"added\": {}, \"deleted\": {}, \"type_changed\": {}, \"binary\": {}}}\n}}\n",
        json_escape(&ctx.project_root.to_string_lossy()),
        json_escape(manager),
        now_unix(),
        manifest_hash(ctx),
        diff.changed,
        diff.added,
        diff.deleted,
        diff.type_changed,
        diff.binary
    );
    fs::write(backup.join("metadata.json"), metadata)
        .map_err(|e| format!("cannot write backup metadata: {e}"))?;
    fs::write(
        backup.join("summary.txt"),
        format!(
            "changed: {}\nadded: {}\ndeleted: {}\ntype_changed: {}\nbinary: {}\n",
            diff.changed, diff.added, diff.deleted, diff.type_changed, diff.binary
        ),
    )
    .map_err(|e| format!("cannot write backup summary: {e}"))?;
    Ok(())
}

impl DiffCounts {
    fn has_changes(&self) -> bool {
        self.changed + self.added + self.deleted + self.type_changed + self.binary > 0
    }

    fn to_json(&self) -> String {
        format!(
            "{{\"changed\":{},\"added\":{},\"deleted\":{},\"type_changed\":{},\"binary\":{}}}",
            self.changed, self.added, self.deleted, self.type_changed, self.binary
        )
    }
}

fn backup_command(ctx: &RimContext, args: &[OsString]) -> Result<u8, String> {
    let Some(sub) = args.first().and_then(|arg| arg.to_str()) else {
        return Err("rim backup requires list, show, or restore".to_owned());
    };
    match sub {
        "list" => backup_list(ctx),
        "show" => {
            let id = args.get(1).and_then(|arg| arg.to_str()).unwrap_or("latest");
            let backup = resolve_backup(ctx, id)?;
            print_backup(&backup)?;
            Ok(0)
        }
        "restore" => backup_restore(ctx, &args[1..]),
        other => Err(format!("unknown backup command: {other}")),
    }
}

fn backup_root(ctx: &RimContext) -> PathBuf {
    ctx.project_root.join(".rim-backups")
}

fn backup_list(ctx: &RimContext) -> Result<u8, String> {
    let mut backups = list_backups(ctx)?;
    backups.sort();
    for backup in backups {
        println!(
            "{}",
            backup
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
        );
    }
    Ok(0)
}

fn list_backups(ctx: &RimContext) -> Result<Vec<PathBuf>, String> {
    let root = backup_root(ctx);
    if !root.exists() {
        return Ok(Vec::new());
    }
    Ok(fs::read_dir(root)
        .map_err(|e| format!("cannot read backup dir: {e}"))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect())
}

fn resolve_backup(ctx: &RimContext, id: &str) -> Result<PathBuf, String> {
    if id == "latest" {
        let mut backups = list_backups(ctx)?;
        backups.sort();
        return backups
            .pop()
            .ok_or_else(|| "no rim backups found".to_owned());
    }
    let backup = backup_root(ctx).join(id);
    if backup.exists() {
        Ok(backup)
    } else {
        Err(format!("backup not found: {id}"))
    }
}

fn print_backup(backup: &Path) -> Result<(), String> {
    let id = backup
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("unknown");
    println!("backup: {id}");
    println!("path: {}", backup.display());
    let metadata = backup.join("metadata.json");
    if metadata.exists() {
        let contents =
            fs::read_to_string(&metadata).map_err(|e| format!("cannot read metadata: {e}"))?;
        if let Some(project_root) = json_string_field(&contents, "project_root") {
            println!("project: {project_root}");
        }
        if let Some(manager) = json_string_field(&contents, "manager") {
            println!("manager: {manager}");
        }
        if let Some(created_at) = json_u64_field(&contents, "created_at") {
            println!("created_at: {created_at}");
        }
        if let Some(manifest_hash) = json_string_field(&contents, "manifest_hash") {
            println!("manifest_hash: {manifest_hash}");
        }
        println!("differences:");
        for key in ["changed", "added", "deleted", "type_changed", "binary"] {
            let value = json_u64_field(&contents, key).unwrap_or(0);
            println!("  {key}: {value}");
        }
    } else {
        let summary = backup.join("summary.txt");
        if summary.exists() {
            print!(
                "{}",
                fs::read_to_string(summary).map_err(|e| format!("cannot read summary: {e}"))?
            );
        }
    }
    println!();
    println!("restore:");
    println!("  rim ensure");
    println!("  rim backup restore {id}");
    Ok(())
}

fn backup_restore(ctx: &RimContext, args: &[OsString]) -> Result<u8, String> {
    let mut id = "latest";
    let mut dry_run = false;
    let mut apply_deletes = false;
    for arg in args {
        match arg.to_str() {
            Some("--dry-run") => dry_run = true,
            Some("--apply-deletes") => apply_deletes = true,
            Some(value) => id = value,
            None => return Err("backup restore args must be valid UTF-8".to_owned()),
        }
    }
    let backup = resolve_backup(ctx, id)?;
    let node_modules = ctx.project_root.join("node_modules");
    if !node_modules.exists() {
        return Err("backup restore requires current project node_modules to exist".to_owned());
    }
    for category in ["changed", "added", "binary"] {
        let src = backup.join(category).join("node_modules");
        if src.exists() {
            restore_tree(&src, &node_modules, dry_run)?;
        }
    }
    if apply_deletes {
        apply_deleted_entries(&backup, &node_modules, dry_run)?;
    } else if backup.join("deleted.json").exists() {
        println!(
            "deleted entries are listed in {}; pass --apply-deletes to remove them",
            backup.join("deleted.json").display()
        );
    }
    println!(
        "restore: {}{}",
        backup.display(),
        if dry_run { " (dry-run)" } else { "" }
    );
    Ok(0)
}

fn restore_tree(src_root: &Path, dst_root: &Path, dry_run: bool) -> Result<(), String> {
    let mut stack = vec![src_root.to_path_buf()];
    while let Some(path) = stack.pop() {
        for entry in fs::read_dir(&path).map_err(|e| format!("cannot read restore tree: {e}"))? {
            let entry = entry.map_err(|e| format!("cannot read restore entry: {e}"))?;
            let src = entry.path();
            let rel = src.strip_prefix(src_root).unwrap_or(&src);
            let dst = dst_root.join(rel);
            let meta =
                fs::symlink_metadata(&src).map_err(|e| format!("cannot stat restore item: {e}"))?;
            if meta.is_dir() && !meta.file_type().is_symlink() {
                stack.push(src);
            } else {
                println!("restore file: {}", dst.display());
                if !dry_run {
                    if let Some(parent) = dst.parent() {
                        fs::create_dir_all(parent)
                            .map_err(|e| format!("cannot create restore parent: {e}"))?;
                    }
                    if meta.file_type().is_symlink() {
                        let target = platform::read_dir_link(&src)
                            .map_err(|e| format!("cannot read backup link: {e}"))?;
                        let _ = platform::remove_dir_link(&dst);
                        platform::create_dir_link(&target, &dst)
                            .map_err(|e| format!("cannot restore directory link: {e}"))?;
                    } else {
                        fs::copy(&src, &dst)
                            .map_err(|e| format!("cannot restore file {}: {e}", dst.display()))?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn apply_deleted_entries(backup: &Path, node_modules: &Path, dry_run: bool) -> Result<(), String> {
    let path = backup.join("deleted.json");
    if !path.exists() {
        return Ok(());
    }
    let contents =
        fs::read_to_string(path).map_err(|e| format!("cannot read deleted.json: {e}"))?;
    for item in parse_json_string_array(&contents) {
        if let Some(rest) = item.strip_prefix("node_modules/") {
            let target = node_modules.join(rest);
            println!("delete: {}", target.display());
            if !dry_run && target.exists() {
                if target.is_dir() && !target.is_symlink() {
                    fs::remove_dir_all(&target)
                        .map_err(|e| format!("cannot delete {}: {e}", target.display()))?;
                } else {
                    fs::remove_file(&target)
                        .map_err(|e| format!("cannot delete {}: {e}", target.display()))?;
                }
            }
        }
    }
    Ok(())
}

fn parse_json_string_array(contents: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut chars = contents.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '"' {
            let mut value = String::new();
            while let Some(ch) = chars.next() {
                match ch {
                    '"' => break,
                    '\\' => {
                        if let Some(next) = chars.next() {
                            value.push(next);
                        }
                    }
                    other => value.push(other),
                }
            }
            values.push(value);
        }
    }
    values
}

fn move_dir_cross_device(src: &Path, dst: &Path) -> Result<(), String> {
    match fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(_) => {
            copy_dir_recursive(src, dst)?;
            fs::remove_dir_all(src)
                .map_err(|e| format!("cannot remove original {} after copy: {e}", src.display()))
        }
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| format!("cannot create {}: {e}", dst.display()))?;
    for entry in fs::read_dir(src).map_err(|e| format!("cannot read {}: {e}", src.display()))? {
        let entry = entry.map_err(|e| format!("cannot read dir entry: {e}"))?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let meta = fs::symlink_metadata(&from)
            .map_err(|e| format!("cannot stat {}: {e}", from.display()))?;
        if meta.file_type().is_symlink() {
            let target = platform::read_dir_link(&from)
                .map_err(|e| format!("cannot read link {}: {e}", from.display()))?;
            platform::create_dir_link(&target, &to)
                .map_err(|e| format!("cannot copy directory link: {e}"))?;
        } else if meta.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            fs::copy(&from, &to).map_err(|e| format!("cannot copy {}: {e}", from.display()))?;
        }
    }
    Ok(())
}

fn expand_tilde(path: &str) -> PathBuf {
    if path == "~"
        && let Some(home) = env::var_os("HOME")
    {
        return PathBuf::from(home);
    }
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(path)
}

fn has_any_lockfile(project_root: &Path) -> bool {
    [
        "bun.lock",
        "bun.lockb",
        "package-lock.json",
        "npm-shrinkwrap.json",
        "pnpm-lock.yaml",
        "yarn.lock",
    ]
    .iter()
    .any(|name| project_root.join(name).exists())
}

fn workspace_detected_in(project_root: &Path) -> bool {
    project_root.join("pnpm-workspace.yaml").exists()
        || project_root.join("turbo.json").exists()
        || project_root.join("rush.json").exists()
        || project_root.join("lerna.json").exists()
        || fs::read_to_string(project_root.join("package.json"))
            .map(|contents| contents.contains("\"workspaces\""))
            .unwrap_or(false)
}

fn lifecycle_scripts_detected_in(project_root: &Path) -> bool {
    fs::read_to_string(project_root.join("package.json"))
        .map(|contents| {
            [
                "\"preinstall\"",
                "\"install\"",
                "\"postinstall\"",
                "\"prepare\"",
            ]
            .iter()
            .any(|needle| contents.contains(needle))
        })
        .unwrap_or(false)
}

fn risky_packages_in(project_root: &Path) -> Vec<&'static str> {
    let package_json = fs::read_to_string(project_root.join("package.json")).unwrap_or_default();
    [
        "next",
        "playwright",
        "electron",
        "expo",
        "react-native",
        "sharp",
        "prisma",
        "puppeteer",
    ]
    .into_iter()
    .filter(|name| package_json.contains(&format!("\"{name}\"")))
    .collect()
}

fn ensure_command(ctx: &RimContext, args: &[OsString], options: CliOptions) -> Result<u8, String> {
    let tool = match args.first().and_then(|arg| arg.to_str()) {
        None => detect_manager(ctx)?,
        Some("bun" | "npm" | "pnpm" | "yarn") => args.first().and_then(|arg| arg.to_str()).unwrap(),
        Some(other) => {
            return Err(format!(
                "unknown ensure manager: {other}; use bun, npm, pnpm, or omit it for auto-detect"
            ));
        }
    };
    ensure_layout_for(ctx, tool)?;
    let reason = dependency_install_reason(ctx);
    write_meta(ctx, tool, false)?;
    if let Some(reason) = reason {
        println!("rim ensure: {reason}; running {tool} install");
        run_install_like(ctx, tool, options)
    } else {
        println!("rim ensure: dependencies already present and manifest hash matches for {tool}");
        Ok(0)
    }
}

fn doctor_command(ctx: &RimContext, args: &[OsString]) -> Result<u8, String> {
    let mut suggest = false;
    for arg in args {
        match arg.to_str() {
            Some("--suggest") => suggest = true,
            Some(other) => return Err(format!("unknown doctor option: {other}")),
            None => return Err("doctor options must be valid UTF-8".to_owned()),
        }
    }
    doctor(ctx);
    if suggest {
        print_suggestions(ctx);
    }
    Ok(0)
}

fn path_command(ctx: &RimContext, args: &[OsString]) -> Result<u8, String> {
    let path = match args.first().and_then(|arg| arg.to_str()) {
        None => &ctx.rim_dir,
        Some("--node-modules") => &ctx.node_modules,
        Some("--cache") => &ctx.rim_dir,
        Some("--npm-cache") => &ctx.npm_cache,
        Some("--bun-cache") => &ctx.bun_cache,
        Some("--deno-cache") => &ctx.deno_dir,
        Some("--tmp") => &ctx.tmp,
        Some("--shadow") => &ctx.shadow_project,
        Some(other) => return Err(format!("unknown path option: {other}")),
    };
    if args.len() > 1 {
        return Err("rim path accepts at most one selector".to_owned());
    }
    println!("{}", path.display());
    Ok(0)
}

fn explain(ctx: &RimContext, args: &[OsString], options: CliOptions) -> Result<u8, String> {
    let Some(first) = args.first().and_then(|arg| arg.to_str()) else {
        return Err("rim explain requires a command, for example: rim explain bun install or rim explain install".to_owned());
    };
    let (tool, tool_args) = if is_manager_shortcut(first) {
        let manager = detect_manager(ctx)?;
        let mut tool_args = Vec::with_capacity(args.len());
        tool_args.push(OsString::from(first));
        tool_args.extend(args[1..].iter().cloned());
        (manager, tool_args)
    } else {
        (first, args[1..].to_vec())
    };
    let install_like = is_install_like(tool, &tool_args);
    let final_args = final_args(ctx, tool, tool_args);
    let cwd = if install_like {
        &ctx.shadow_project
    } else {
        &ctx.project_root
    };

    println!("tool: {tool}");
    println!("args: {}", join_args(&final_args));
    println!("install_like: {install_like}");
    println!("cwd: {}", cwd.display());
    println!("rim_dir: {}", ctx.rim_dir.display());
    println!();
    println!("rim will:");
    println!("  1. prepare dependency-layer layout and metadata");
    if install_like {
        println!("  2. sync manifests to the shadow project");
        println!(
            "  3. run `{tool} {}` in the shadow project",
            join_args(&final_args)
        );
        println!("  4. copy mutable manifests and lockfiles back on success");
        if matches!(tool, "npm" | "bun") && !options.keep_cache {
            println!("  5. trim {tool} cache unless --keep-cache is set");
        } else if options.keep_cache {
            println!("  5. keep package-manager cache because --keep-cache is set");
        }
    } else {
        println!(
            "  2. run `{tool} {}` in the real project",
            join_args(&final_args)
        );
        println!("  3. leave source files in place and dependency mass outside the project");
    }
    if options.auto_clean || options.ephemeral {
        println!("  cleanup. remove the dependency layer after the command exits");
    }
    if tool == "pnpm" {
        println!();
        println!(
            "warning: pnpm support is experimental and may use significantly more RAM for its store."
        );
    }
    Ok(0)
}

fn current_project_has_broken_rim_link(ctx: &RimContext) -> bool {
    let link = ctx.project_root.join("node_modules");
    platform::is_dir_link(&link)
        && platform::read_dir_link(&link)
            .map(|target| target.starts_with(&ctx.rim_base) && !target.exists())
            .unwrap_or(false)
}

fn print_suggestions(ctx: &RimContext) {
    println!();
    println!("suggestions:");
    let mut any = false;
    if let Some(info) = storage_info_for(&ctx.rim_base)
        && info.available_bytes < 1024 * 1024 * 1024
    {
        any = true;
        println!(
            "  - RIM_BASE has only {} available: {}",
            format_bytes(info.available_bytes),
            ctx.rim_base.display()
        );
        println!("    Try disk-backed mode:");
        println!("      RIM_BASE=$HOME/.cache/rim rim bun install");
    }
    let risky = risky_packages(ctx);
    if !risky.is_empty() {
        any = true;
        println!("  - heavy packages detected: {}", risky.join(", "));
        println!("    tmpfs mode may be too large; consider $HOME/.cache/rim or external storage.");
    }
    if workspace_detected(ctx) {
        any = true;
        println!("  - workspace detected; shadow installs may need extra care.");
    }
    if lifecycle_scripts_detected(ctx) {
        any = true;
        println!(
            "  - lifecycle scripts detected; postinstall/prepare hooks may assume real project cwd."
        );
    }
    let stale_locks = collect_layers(ctx)
        .into_iter()
        .filter(|layer| matches!(layer.active, ActiveState::Stale(_)))
        .count();
    if stale_locks > 0 {
        any = true;
        println!("  - stale active lock(s) detected: {stale_locks}");
        println!("    Try: rim repair --stale-locks --dry-run");
    }
    if current_project_has_broken_rim_link(ctx) {
        any = true;
        println!("  - broken rim-managed node_modules link detected");
        println!("    Try: rim repair --broken-links --dry-run");
        println!("    Then: rim ensure");
    }
    if rim_mode(ctx) == "tmpfs" && dir_size(&ctx.rim_base).unwrap_or(0) > 1024 * 1024 * 1024 {
        any = true;
        println!("  - RIM_BASE already contains more than 1 GB of layers.");
        println!("    Try: rim ls");
        println!("    Then: rim gc --dry-run --orphaned");
    }
    if !any {
        println!("  - No obvious issues detected. npm/bun cache trim is enabled by default.");
    }
}

fn risky_packages(ctx: &RimContext) -> Vec<&'static str> {
    let package_json =
        fs::read_to_string(ctx.project_root.join("package.json")).unwrap_or_default();
    [
        "next",
        "playwright",
        "electron",
        "expo",
        "react-native",
        "sharp",
        "prisma",
        "puppeteer",
    ]
    .into_iter()
    .filter(|name| package_json.contains(&format!("\"{name}\"")))
    .collect()
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
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(err),
    };

    let mut total = meta.len();
    if meta.is_dir() && !meta.file_type().is_symlink() {
        for entry in fs::read_dir(path)? {
            total += dir_size(&entry?.path())?;
        }
    }
    Ok(total)
}

fn is_manager_shortcut(command: &str) -> bool {
    matches!(
        command,
        "install"
            | "i"
            | "add"
            | "remove"
            | "rm"
            | "update"
            | "up"
            | "ci"
            | "run"
            | "test"
            | "start"
    )
}

#[derive(Debug, Clone, Copy)]
struct ManagerDetection {
    manager: &'static str,
    reason: &'static str,
}

fn detect_manager(ctx: &RimContext) -> Result<&'static str, String> {
    Ok(detect_manager_with_reason(ctx)?.manager)
}

fn detect_manager_with_reason(ctx: &RimContext) -> Result<ManagerDetection, String> {
    let package_json =
        fs::read_to_string(ctx.project_root.join("package.json")).unwrap_or_default();
    if let Some(package_manager) = json_string_field(&package_json, "packageManager") {
        if package_manager.starts_with("bun@") {
            return Ok(ManagerDetection {
                manager: "bun",
                reason: "packageManager bun@...",
            });
        }
        if package_manager.starts_with("npm@") {
            return Ok(ManagerDetection {
                manager: "npm",
                reason: "packageManager npm@...",
            });
        }
        if package_manager.starts_with("pnpm@") {
            eprintln!("rim: warning: auto-detected pnpm, which is experimental in rim.");
            return Ok(ManagerDetection {
                manager: "pnpm",
                reason: "packageManager pnpm@...",
            });
        }
    }
    if ctx.project_root.join("bun.lock").exists() {
        return Ok(ManagerDetection {
            manager: "bun",
            reason: "bun.lock found",
        });
    }
    if ctx.project_root.join("bun.lockb").exists() {
        return Ok(ManagerDetection {
            manager: "bun",
            reason: "bun.lockb found",
        });
    }
    if ctx.project_root.join("package-lock.json").exists() {
        return Ok(ManagerDetection {
            manager: "npm",
            reason: "package-lock.json found",
        });
    }
    if ctx.project_root.join("pnpm-lock.yaml").exists() {
        eprintln!("rim: warning: auto-detected pnpm, which is experimental in rim.");
        return Ok(ManagerDetection {
            manager: "pnpm",
            reason: "pnpm-lock.yaml found",
        });
    }
    if ctx.project_root.join("deno.json").exists() || ctx.project_root.join("deno.jsonc").exists() {
        return Err("manager auto-detection found Deno, but rim install/run shortcuts target package managers. Use `rim deno ...` directly.".to_owned());
    }
    if ctx.project_root.join("package.json").exists() {
        return Ok(ManagerDetection {
            manager: "bun",
            reason: "package.json only defaults to bun",
        });
    }
    Err(
        "cannot auto-detect package manager; use `rim bun ...`, `rim npm ...`, or `rim deno ...`"
            .to_owned(),
    )
}

fn run_tool(
    ctx: &RimContext,
    tool: &str,
    args: Vec<OsString>,
    options: CliOptions,
) -> Result<u8, String> {
    let install_like = is_install_like(tool, &args);
    if (options.auto_clean || options.ephemeral) && install_like {
        eprintln!("rim: warning: cleanup after install will remove installed dependencies.");
        eprintln!("rim: manifest and lockfile changes will remain.");
    }
    if install_like {
        warn_about_low_rim_space(ctx);
    }
    if tool == "pnpm" {
        eprintln!(
            "rim: warning: pnpm support is experimental and may use significantly more RAM for its store."
        );
    }

    if options.ephemeral && !options.dry_run {
        clean(ctx)?;
    }

    ensure_layout_for(ctx, tool)?;
    let install_reason = dependency_install_reason(ctx);
    write_meta(ctx, tool, false)?;

    let needs_ephemeral_install = options.ephemeral
        && !install_like
        && should_ephemeral_install(tool, &args)
        && (options.dry_run || install_reason.is_some());

    if needs_ephemeral_install {
        let install_code = run_install_like(ctx, tool, options)?;
        if install_code != 0 {
            if options.should_clean_after(install_code) {
                clean_after_command(ctx);
            }
            return Ok(install_code);
        }
    }

    let needs_shortcut_ensure = options.ensure_before_run
        && !install_like
        && should_ephemeral_install(tool, &args)
        && (options.dry_run || install_reason.is_some());
    if needs_shortcut_ensure && !options.dry_run {
        let install_code = run_install_like(ctx, tool, options)?;
        if install_code != 0 {
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
        println!("keep_cache={}", options.keep_cache);
        if needs_ephemeral_install {
            println!("ephemeral_install: {} install", tool);
        }
        if needs_shortcut_ensure {
            println!("ensure_install: {} install", tool);
        }
        print_env(ctx);
        return Ok(0);
    }

    let exit_code = run_command(ctx, tool, &final_args, cwd)?;

    if install_like && exit_code == 0 {
        sync_mutated_manifests_back(ctx)?;
        trim_install_cache(ctx, tool, options);
        write_meta(ctx, tool, true)?;
    }

    if options.should_clean_after(exit_code) {
        clean_after_command(ctx);
    }

    Ok(exit_code)
}

fn run_install_like(ctx: &RimContext, tool: &str, options: CliOptions) -> Result<u8, String> {
    warn_about_low_rim_space(ctx);
    sync_manifests_to_shadow(ctx)?;
    let args = final_args(ctx, tool, vec![OsString::from("install")]);

    if options.dry_run {
        return Ok(0);
    }

    let code = run_command(ctx, tool, &args, &ctx.shadow_project)?;
    if code == 0 {
        sync_mutated_manifests_back(ctx)?;
        trim_install_cache(ctx, tool, options);
        write_meta(ctx, tool, true)?;
    }
    Ok(code)
}

fn run_command(ctx: &RimContext, tool: &str, args: &[OsString], cwd: &Path) -> Result<u8, String> {
    write_active_lock(ctx, tool, args)?;
    let _signals = platform::SignalGuard::install();
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

    platform::configure_child_command(&mut command);

    let status = command.status();
    remove_active_lock(ctx);
    let status = status.map_err(|e| format!("failed to execute {tool}: {e}"))?;
    Ok(platform::exit_status_code(status))
}

fn clean_after_command(ctx: &RimContext) {
    if let Err(err) = clean(ctx) {
        eprintln!("rim: warning: auto-clean failed: {err}");
    }
}

fn trim_install_cache(ctx: &RimContext, tool: &str, options: CliOptions) {
    if options.keep_cache {
        return;
    }
    let cache_dir = match tool {
        "npm" => Some(&ctx.npm_cache),
        "bun" => Some(&ctx.bun_cache),
        _ => None,
    };
    if let Some(cache_dir) = cache_dir
        && cache_dir.exists()
        && let Err(err) = fs::remove_dir_all(cache_dir)
    {
        eprintln!(
            "rim: warning: failed to trim {} cache {}: {err}",
            tool,
            cache_dir.display()
        );
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

fn dependency_install_reason(ctx: &RimContext) -> Option<String> {
    if dependencies_missing(ctx) {
        return Some("dependencies missing".to_owned());
    }
    let Some(meta) = read_meta(&ctx.rim_dir) else {
        return Some("manifest metadata missing".to_owned());
    };
    let current = manifest_hash(ctx);
    match meta.manifest_hash {
        Some(stored) if stored == current => None,
        Some(_) => Some("manifest hash changed".to_owned()),
        None => Some("manifest hash missing".to_owned()),
    }
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

fn manifest_hash(ctx: &RimContext) -> String {
    let mut hasher = DefaultHasher::new();
    for name in manifest_names() {
        name.hash(&mut hasher);
        let path = ctx.project_root.join(name);
        match fs::read(&path) {
            Ok(bytes) => bytes.hash(&mut hasher),
            Err(_) => 0_u8.hash(&mut hasher),
        }
    }
    format!("{:016x}", hasher.finish())
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
