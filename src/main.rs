use std::env;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

#[derive(Debug, Clone)]
struct RimContext {
    project_root: PathBuf,
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
    let dry_run = if args.first().is_some_and(|a| a == "--dry-run") {
        args.remove(0);
        true
    } else {
        false
    };

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
        "help" | "--help" | "-h" => {
            print_help();
            Ok(0)
        }
        tool => {
            let tool_args = args.split_off(1);
            run_tool(&ctx, tool, tool_args, dry_run)
        }
    }
}

fn print_help() {
    println!(
        "rim - RAM dependency wrapper\n\nUsage:\n  rim prepare\n  rim status\n  rim clean\n  rim [--dry-run] <bun|npm|pnpm|deno|node|...> [args...]\n\nEnvironment:\n  RIM_BASE   RAM base directory, default /dev/shm/rim"
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
    println!("rim_dir: {}", ctx.rim_dir.display());
    println!("shadow_project: {}", ctx.shadow_project.display());
    println!("node_modules: {}", ctx.node_modules.display());
    let bytes = dir_size(&ctx.rim_dir).unwrap_or(0);
    println!("rim_size_bytes: {bytes}");
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
    dry_run: bool,
) -> Result<u8, String> {
    let install_like = is_install_like(tool, &args);
    ensure_layout(ctx)?;
    if install_like {
        sync_manifests_to_shadow(ctx)?;
    }

    let final_args = final_args(ctx, tool, args);
    let cwd = if install_like {
        &ctx.shadow_project
    } else {
        &ctx.project_root
    };

    if dry_run {
        println!("project: {}", ctx.project_root.display());
        println!("rim_dir: {}", ctx.rim_dir.display());
        println!("cwd: {}", cwd.display());
        println!("command: {} {}", tool, join_args(&final_args));
        println!("npm_config_cache={}", ctx.npm_cache.display());
        println!("XDG_CACHE_HOME={}", ctx.xdg_cache.display());
        println!("TMPDIR={}", ctx.tmp.display());
        println!("DENO_DIR={}", ctx.deno_dir.display());
        println!(
            "PLAYWRIGHT_BROWSERS_PATH={}",
            ctx.playwright_browsers.display()
        );
        println!("BUN_INSTALL_CACHE_DIR={}", ctx.bun_cache.display());
        return Ok(0);
    }

    let status = Command::new(tool)
        .args(&final_args)
        .current_dir(cwd)
        .env("npm_config_cache", &ctx.npm_cache)
        .env("XDG_CACHE_HOME", &ctx.xdg_cache)
        .env("TMPDIR", &ctx.tmp)
        .env("DENO_DIR", &ctx.deno_dir)
        .env("PLAYWRIGHT_BROWSERS_PATH", &ctx.playwright_browsers)
        .env("BUN_INSTALL_CACHE_DIR", &ctx.bun_cache)
        .status()
        .map_err(|e| format!("failed to execute {tool}: {e}"))?;

    if install_like && status.success() {
        sync_mutated_manifests_back(ctx)?;
    }

    Ok(status.code().unwrap_or(1) as u8)
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
    println!("rim_dir: {}", ctx.rim_dir.display());
    println!("shadow_project: {}", ctx.shadow_project.display());
    println!(
        "link: {} -> {}",
        ctx.project_root.join("node_modules").display(),
        ctx.node_modules.display()
    );
}
