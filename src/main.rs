use std::env;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::fs::symlink;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, ExitStatus};
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
            write_meta(&ctx, "prepare")?;
            print_context(&ctx);
            Ok(0)
        }
        "clean" => {
            let clean_args = args.split_off(1);
            clean_command(&ctx, &clean_args)
        }
        "ensure" => {
            let ensure_args = args.split_off(1);
            ensure_command(&ctx, &ensure_args, options)
        }
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
  rim clean [--cache-only|--deps-only]
  rim ensure [bun|npm|pnpm]
  rim ls
  rim gc [--dry-run] [--orphaned] [--older-than 1d] [--all]
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
  RIM_BASE      dependency layer base directory, default /dev/shm/rim
  RIM_PROFILE   ram|cache|external preset when RIM_BASE is unset"
    );
}

fn resolve_rim_base() -> Result<PathBuf, String> {
    if let Some(base) = env::var_os("RIM_BASE") {
        return Ok(PathBuf::from(base));
    }
    match env::var("RIM_PROFILE").ok().as_deref() {
        None | Some("") | Some("ram") => Ok(PathBuf::from("/dev/shm/rim")),
        Some("cache") => Ok(cache_dir().join("rim")),
        Some("external") => Ok(env::var_os("RIM_EXTERNAL_BASE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/mnt/external/rim"))),
        Some(other) => Err(format!(
            "unknown RIM_PROFILE={other}; expected ram, cache, or external"
        )),
    }
}

fn build_context() -> Result<RimContext, String> {
    let cwd = env::current_dir().map_err(|e| format!("cannot read cwd: {e}"))?;
    let project_root = find_project_root(&cwd);
    let base = resolve_rim_base()?;
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
            "node_modules exists and is not a symlink. Try: mv node_modules node_modules.backup && rim install"
                .to_owned(),
        ),
        Err(e) if e.kind() == io::ErrorKind::NotFound => symlink(&ctx.node_modules, &link)
            .map_err(|e| format!("cannot create node_modules symlink: {e}")),
        Err(e) => Err(format!("cannot inspect node_modules: {e}")),
    }
}

fn clean_command(ctx: &RimContext, args: &[OsString]) -> Result<u8, String> {
    let mut cache_only = false;
    let mut deps_only = false;
    for arg in args {
        match arg.to_str() {
            Some("--cache-only") => cache_only = true,
            Some("--deps-only") => deps_only = true,
            Some(other) => return Err(format!("unknown clean option: {other}")),
            None => return Err("clean options must be valid UTF-8".to_owned()),
        }
    }
    if cache_only && deps_only {
        return Err("--cache-only and --deps-only cannot be used together".to_owned());
    }
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
struct RimMeta {
    project_root: String,
    created_at: u64,
    last_used_at: u64,
    manager: String,
    mode: String,
    rim_version: String,
}

#[derive(Debug, Clone)]
struct LayerInfo {
    rim_dir: PathBuf,
    meta: Option<RimMeta>,
    size_bytes: u64,
}

#[derive(Debug, Clone, Default)]
struct GcOptions {
    dry_run: bool,
    all: bool,
    orphaned: bool,
    older_than_seconds: Option<u64>,
}

fn write_meta(ctx: &RimContext, manager: &str) -> Result<(), String> {
    let now = now_unix();
    let existing = read_meta(&ctx.rim_dir);
    let created_at = existing.as_ref().map_or(now, |meta| meta.created_at);
    let contents = format!(
        "{{\n  \"project_root\": \"{}\",\n  \"created_at\": {},\n  \"last_used_at\": {},\n  \"manager\": \"{}\",\n  \"mode\": \"{}\",\n  \"rim_version\": \"{}\"\n}}\n",
        json_escape(&ctx.project_root.to_string_lossy()),
        created_at,
        now,
        json_escape(manager),
        json_escape(&rim_mode(ctx)),
        json_escape(env!("CARGO_PKG_VERSION"))
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
    })
}

fn list_layers(ctx: &RimContext) {
    let layers = collect_layers(ctx);
    println!(
        "{:<36} {:<10} {:<8} {:>10} {:>8} {:>10} {:<8}  LAYER",
        "PROJECT", "MANAGER", "MODE", "SIZE", "AGE", "LAST_USED", "VERSION"
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
        println!(
            "{project:<36} {manager:<10} {mode:<8} {:>10} {:>8} {:>10} {version:<8}  {}",
            format_bytes(layer.size_bytes),
            age,
            last_used,
            layer.rim_dir.display()
        );
    }
}

fn gc(ctx: &RimContext, args: &[OsString]) -> Result<u8, String> {
    let mut options = parse_gc_options(args)?;
    if !options.all && !options.orphaned && options.older_than_seconds.is_none() {
        options.dry_run = true;
        options.orphaned = true;
        println!("rim gc: defaulting to --dry-run --orphaned");
    }

    let now = now_unix();
    let mut matched = 0_u64;
    let mut bytes = 0_u64;
    for layer in collect_layers(ctx) {
        if !gc_matches(&layer, &options, now) {
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
            "--older-than" => {
                i += 1;
                let Some(value) = args.get(i).and_then(|arg| arg.to_str()) else {
                    return Err("--older-than requires a value like 1d, 6h, or 30m".to_owned());
                };
                options.older_than_seconds = Some(parse_duration_seconds(value)?);
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

fn gc_matches(layer: &LayerInfo, options: &GcOptions, now: u64) -> bool {
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
    write_meta(ctx, tool)?;
    if dependencies_missing(ctx) {
        println!("rim ensure: dependencies missing; running {tool} install");
        run_install_like(ctx, tool, options)
    } else {
        println!("rim ensure: dependencies already present for {tool}");
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

fn detect_manager(ctx: &RimContext) -> Result<&'static str, String> {
    let package_json =
        fs::read_to_string(ctx.project_root.join("package.json")).unwrap_or_default();
    if ctx.project_root.join("bun.lock").exists()
        || ctx.project_root.join("bun.lockb").exists()
        || package_json.contains("\"packageManager\":") && package_json.contains("bun@")
    {
        return Ok("bun");
    }
    if ctx.project_root.join("package-lock.json").exists()
        || package_json.contains("\"packageManager\":") && package_json.contains("npm@")
    {
        return Ok("npm");
    }
    if ctx.project_root.join("pnpm-lock.yaml").exists()
        || package_json.contains("\"packageManager\":") && package_json.contains("pnpm@")
    {
        eprintln!("rim: warning: auto-detected pnpm, which is experimental in rim.");
        return Ok("pnpm");
    }
    if ctx.project_root.join("deno.json").exists() || ctx.project_root.join("deno.jsonc").exists() {
        return Err("manager auto-detection found Deno, but rim install/run shortcuts target package managers. Use `rim deno ...` directly.".to_owned());
    }
    if ctx.project_root.join("package.json").exists() {
        return Ok("bun");
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
    write_meta(ctx, tool)?;

    let needs_ephemeral_install = options.ephemeral
        && !install_like
        && should_ephemeral_install(tool, &args)
        && (options.dry_run || dependencies_missing(ctx));

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
        && (options.dry_run || dependencies_missing(ctx));
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

const SIGINT: i32 = 2;
const SIGTERM: i32 = 15;
const SIG_DFL: usize = 0;

type SignalHandler = usize;

unsafe extern "C" {
    fn signal(signum: i32, handler: SignalHandler) -> SignalHandler;
}

extern "C" fn record_signal(_signal: i32) {}

struct SignalGuard {
    previous_int: SignalHandler,
    previous_term: SignalHandler,
}

impl SignalGuard {
    fn install() -> Self {
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
