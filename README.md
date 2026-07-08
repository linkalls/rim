# rim

**rim** is a tiny Rust CLI for storage-starved Linux/WSL machines: keep project source on disk, but move disposable JavaScript dependency weight into a separate dependency layer.

The core idea:

> Keep source. Evaporate dependencies.

`rim npm install` / `rim bun install` prepares a shadow project under `RIM_BASE` (`/dev/shm/rim` by default), installs dependencies there, trims disposable package-manager cache after successful installs, and leaves your real project with only a `node_modules` symlink.

This is especially useful on small experiments, AI-generated throwaway apps, and mini PCs where 32-64 GB eMMC/SSD storage is more precious than disposable dependency state.

## Current status

MVP, Linux-first and WSL-friendly. Native Windows builds are compile-only and intentionally refuse to run.

Implemented:

- `rim prepare`
- `rim status`
- `rim doctor`
- `rim doctor --suggest`
- `rim scan`
- `rim adopt`
- `rim adopt --diff-backup`
- `rim backup list/show/restore`
- `rim ls`
- `rim gc`
- `rim pin` / `rim unpin`
- `rim manager`
- `rim path`
- `rim explain`
- manager shortcuts: `rim install`, `rim run dev`, `rim test`
- `rim ensure`
- `rim clean`
- `rim clean --cache-only`
- `rim clean --deps-only`
- active layer locks: `.rim-active`, `rim clean --force`, `rim gc --force`
- `rim repair --stale-locks` / `rim repair --broken-links`
- `rim --auto-clean ...`
- `rim --ephemeral ...`
- `rim [--dry-run] npm ...`
- `rim [--dry-run] bun ...`
- `rim [--dry-run] deno ...` cache env wiring
- `RIM_PROFILE=ram|cache|external` presets when `RIM_BASE` is unset
- RAM-backed npm cache / Deno cache / Bun cache envs
- install-like commands run in a shadow project so npm cannot replace the project symlink with a real directory
- generated lockfiles are copied back to the real project after successful install-like commands
- Deno commands skip the project `node_modules` symlink and only redirect cache/env paths
- `.rim-meta.json` is written into each dependency layer for `rim ls` / `rim gc`, with manifest hash and pinned state
- existing unmanaged `node_modules` can be scanned and safely adopted into a rim layer
- optional diff backups compare existing `node_modules` against a fresh install
- `bunfig.toml` is copied into the shadow project for Bun installs
- native Windows compile support via a small platform abstraction; native Windows runtime is intentionally disabled

No external Rust dependencies are used.

## Install locally

```bash
cargo install --path .
```

Or build directly:

```bash
cargo build --release
./target/release/rim --help
```

## Usage

In a project with `package.json`:

```bash
rim npm install
rim npm run dev
```

For Bun:

```bash
rim bun install
rim bun run dev
```

Or let `rim` detect the package manager from lockfiles / `packageManager`:

```bash
rim install
rim run dev
rim test
```

Detection order gives `packageManager` first priority, then Bun lockfiles, then npm lockfiles, then pnpm as experimental. If only `package.json` exists, `rim` defaults to Bun.

`rim run`, `rim test`, and `rim start` also ensure dependencies first when the dependency layer is missing or stale:

```bash
rim run dev   # auto-detect manager, install if missing/stale, then run dev
rim ensure    # install if missing/stale, otherwise do nothing
rim ensure bun
rim manager   # show detected manager and why
```

For Deno, `rim` mainly redirects caches and does not create a project `node_modules` symlink:

```bash
rim deno run --allow-net main.ts
```

Inspect current mapping:

```bash
rim status
```

Check storage/memory risk before installing:

```bash
rim doctor
rim doctor --suggest
```

List dependency layers under the current `RIM_BASE`:

```bash
rim ls
```

Garbage-collect old or orphaned dependency layers:

```bash
rim gc --dry-run --orphaned
rim gc --orphaned
rim gc --dry-run --older-than 1d
rim gc --older-than 1d
rim gc --dry-run --all
rim gc --dry-run --all --include-pinned
rim gc --all --force            # override active-lock protection
```

`rim gc` with no selector is safe by default: it behaves like `rim gc --dry-run --orphaned`.

Find unmanaged `node_modules` directories and adopt one safely:

```bash
rim scan ~/code
rim scan --json ~/code
rim adopt ~/code/tiny-hono --dry-run
rim adopt ~/code/tiny-hono
rim adopt ~/code/tiny-hono --diff-backup
rim adopt ~/code/tiny-hono --copy
```

Manage diff backups created by `--diff-backup`:

```bash
rim backup list
rim backup show latest
rim backup restore latest --dry-run
rim backup restore latest
```

Repair stale active locks left by dead processes, or remove a broken rim-managed `node_modules` link after `/dev/shm` was cleared:

```bash
rim repair --stale-locks --dry-run
rim repair --stale-locks
rim repair --broken-links --dry-run
rim repair --broken-links
rim ensure
```

Remove the current project's dependency directory and `node_modules` symlink:

```bash
rim clean
```

Or remove only one part of the current layer:

```bash
rim clean --cache-only  # keep node_modules, remove cache dirs
rim clean --deps-only   # keep cache dirs, remove node_modules layer + project symlink
rim clean --force       # override active-lock protection
```

Dry-run without executing the wrapped tool:

```bash
rim --dry-run npm install
```

Dry-run still prepares the dependency-layer layout, `node_modules` symlink, and metadata so the printed paths/env match a real run. Shortcut run/test/start dry-runs also show `ensure_install: <manager> install` when dependencies are missing.

Print script-friendly paths:

```bash
rim path
rim path --node-modules
rim path --npm-cache
rim path --bun-cache
rim path --deno-cache
rim path --tmp
rim path --shadow
```

Explain what `rim` would do for a command:

```bash
rim explain install
rim explain bun install
rim explain bun run dev
```

`rim explain` does not create or remove anything; it only describes the plan. It also accepts shortcuts such as `rim explain install` and resolves the manager using the same auto-detection rules.

## Auto-clean and ephemeral mode

`--auto-clean` is the low-level cleanup hook. It uses the current `rim` dependency layer, runs the wrapped command, then runs the same cleanup as `rim clean` after the command exits.

```bash
rim bun install
rim --auto-clean bun run dev
rim --auto-clean bun test
```

Notes:

- `--auto-clean` does not auto-install dependencies.
- The wrapped command's exit code is preserved.
- By default, cleanup runs after both success and failure.
- Add `--keep-on-error` to preserve the dependency layer after a failing command.

```bash
rim --auto-clean --keep-on-error bun test
```

If you use cleanup mode with an install-like command, `rim` warns because the installed dependency tree will be removed immediately after install finishes. Manifest and lockfile changes are still copied back to the real project.

```bash
rim --auto-clean bun install
# warning: dependencies will be removed, package/lockfile changes remain
```

`--ephemeral` is the higher-level one-shot experiment mode. It implies `--auto-clean`, starts from a fresh dependency layer, auto-runs `install` for package-manager run/test/start commands when dependencies are missing, runs the requested command, then cleans up.

```bash
rim --ephemeral bun run dev
rim --ephemeral bun test
rim --ephemeral npm run dev
```

This is the intended mode for cloning a small repo, trying it once, then leaving only source and lockfile changes behind. `Ctrl+C`/SIGINT is best-effort handled so `rim` can usually clean after the child exits.

## How it works

For a project like:

```txt
/home/poteto/code/app
```

`rim` creates a minimal base layout, then package-manager cache directories appear only when the wrapped tool uses them:

```txt
/dev/shm/rim/app-<hash>/
  .rim-meta.json
  project/
    package.json
    package-lock.json
    bunfig.toml
    node_modules/
  npm-cache/    # created only when npm uses it; trimmed after successful npm installs by default
  deno-cache/
  bun-cache/    # created only when bun uses it; trimmed after successful bun installs by default
  xdg-cache/
  tmp/
```

The real project gets:

```txt
node_modules -> /dev/shm/rim/app-<hash>/project/node_modules
```

Each layer also gets `.rim-meta.json` so `rim ls` and `rim gc` can reverse-map a dependency layer back to its source project, manager, mode, created time, last-used time, manifest hash, and pinned state. Metadata writes are atomic: `rim` writes a temporary file and renames it into place.

While a wrapped command is running, `rim` writes `.rim-active` in the layer with the current pid and command. This is best-effort active protection: on Linux, `rim` treats `/proc/<pid>` as the source of truth. `rim clean` refuses active layers by default, and `rim gc` skips them. Use `--force` only when you are sure the process is gone or the lock is stale.

If a process dies and leaves a stale lock, `rim doctor --suggest` reports it and `rim repair --stale-locks` removes only locks whose pid is no longer alive. If `/dev/shm` was cleared and the project still has a rim-managed `node_modules` symlink pointing at a missing layer, `rim doctor --suggest` reports it and `rim repair --broken-links` removes only that broken project link. Run `rim ensure` afterward to regenerate dependencies.

Deno commands are special: `rim deno ...` creates only the dependency-layer metadata/cache root and does not create a `node_modules` symlink in the project.

For install-like commands (`install`, `i`, `add`, `remove`, `rm`, `update`, `up`, `ci`), `rim` runs the package manager inside the RAM shadow project. This matters because `npm install` may replace a pre-existing `node_modules` symlink if it is run directly in the source project.

For non-install commands (`run dev`, `test`, etc.), `rim` runs from the real project so relative source paths behave normally.

`rim ensure` is the reusable dependency check: it prepares the layer, installs when `node_modules` is missing/empty or when the manifest hash changed, and otherwise exits without running the package manager. Shortcut commands (`rim run`, `rim test`, `rim start`) use the same hash-aware check before running.

## Environment

`RIM_BASE` controls where the dependency layer is stored. If `RIM_BASE` is unset, `RIM_PROFILE` can choose a preset.

Default RAM/tmpfs mode:

```bash
RIM_BASE=/dev/shm/rim rim npm install
```

Profile presets:

```bash
RIM_PROFILE=ram rim install       # Linux/WSL: /dev/shm/rim
RIM_PROFILE=cache rim install     # Linux/WSL: $HOME/.cache/rim
RIM_PROFILE=external rim install  # requires $RIM_EXTERNAL_BASE
```

Default when unset:

```txt
Linux/WSL:       /dev/shm/rim
Native Windows:  unsupported at runtime; use WSL
```

You can choose different operating modes:

| Mode | Example | Tradeoff |
|---|---|---|
| RAM/tmpfs | `RIM_BASE=/dev/shm/rim` | Fast and fully disposable; uses Linux/WSL RAM/shared memory |
| Cache isolation | `RIM_BASE=$HOME/.cache/rim` | Saves project directories from `node_modules`; uses normal disk cache |
| External storage | `RIM_BASE=/mnt/external/rim` | Good for tiny internal disks; dependency mass goes to USB/SSD |

So `rim` is not only a RAM trick. The main invariant is: source stays in the project, dependency mass lives somewhere else.

`rim` injects:

```txt
npm_config_cache=<rim-dir>/npm-cache
XDG_CACHE_HOME=<rim-dir>/xdg-cache
TMPDIR=<rim-dir>/tmp
DENO_DIR=<rim-dir>/deno-cache
PLAYWRIGHT_BROWSERS_PATH=<rim-dir>/playwright-browsers
BUN_INSTALL_CACHE_DIR=<rim-dir>/bun-cache
```

`rim` can still run arbitrary commands, but pnpm is not a recommended target for the RAM/tmpfs mode. Its store model showed large RAM overhead in benchmarking, so pnpm is treated as experimental/opt-in rather than part of the main support path.

After successful npm/bun install-like commands, `rim` trims the package-manager cache directory by default. The installed `node_modules` dependency tree remains. Use `--keep-cache` if you prefer faster repeated installs over minimum RAM usage.


## Windows

Native Windows builds are compile-only for now and intentionally refuse to run.

`rim` is Linux-first. For Windows machines, use WSL to get the intended `/dev/shm` RAM/tmpfs behavior:

```bash
RIM_PROFILE=ram rim install
```

Why native Windows is disabled:

- directory symlinks depend on Developer Mode, `SeCreateSymbolicLinkPrivilege`, or Administrator privileges
- RAM-disk behavior is not built into Windows and would require user-created drive setup
- active lock checks are only best-effort outside Linux/WSL
- WSL already gives the clean Linux semantics rim is designed around

The Windows platform abstraction stays in the codebase so the project can continue to compile for `x86_64-pc-windows-msvc`, but runtime support is not recommended until there is a stronger reason to maintain it.

## Scanning and adopting existing node_modules

`rim scan` searches for unmanaged `node_modules` directories under one or more paths:

```bash
rim scan ~/code
rim scan ~/code ~/Downloads
rim scan --json ~/code
rim scan --json --diff ~/code/single-project
```

It skips obvious junk directories such as `.git`, `.rim-backups`, `target`, `dist`, `build`, and it does not recurse inside `node_modules`. Each candidate reports the project path, size, inferred manager, risk, action, and warnings:

```txt
PROJECT                         SIZE      MANAGER  RISK    ACTION     WARNINGS
~/code/tiny-hono                18.3 MB   bun      low     adoptable
~/code/next-test                550 MB    npm      medium  review     heavy packages detected: next
~/code/pnpm-mono                2.8 GB    pnpm     high    skip       pnpm store layout is high-risk for adopt
```

`rim adopt <project>` moves an existing real `node_modules` directory into the current `RIM_BASE` layer and replaces the project copy with a symlink. If adoption fails after the move, `rim` tries to roll back by moving `node_modules` back to the project. If rollback also fails, it prints the exact recovery path and manual `ln -s` / `mv` commands:

```bash
rim adopt ~/code/tiny-hono --dry-run
rim adopt ~/code/tiny-hono
```

High-risk candidates are refused by default. pnpm, workspaces, symlinked/broken `node_modules`, and projects without `package.json` are high-risk. Use `--allow-risk` only after reviewing the warnings:

```bash
rim adopt ~/code/pnpm-mono --allow-risk
```

Adopting into `/dev/shm` is intentionally loud: adopted `node_modules` will disappear on reboot. This is fine for reproducible dependencies, but unsafe for hand-edited `node_modules` unless you keep the layer on disk or save a delta backup.

Prefer one of these before adopting anything you cannot regenerate:

```bash
RIM_PROFILE=cache rim adopt ~/code/app
RIM_PROFILE=external rim adopt ~/code/app
rim adopt ~/code/app --diff-backup
rim adopt ~/code/app --copy
```

`rim adopt --copy` is useful when you still want the dependency layer in RAM, but do not want to lose the original hand-edited `node_modules`. It copies `node_modules` into the rim layer, moves the original directory to `.rim-backups/node_modules-original-<timestamp>/`, then creates the project symlink. This temporarily uses roughly 2x `node_modules` space.

`rim adopt --diff-backup` creates a scratch fresh install from the same manifest/lockfile, compares it against the existing `node_modules`, and stores only the delta under `.rim-backups/`:

```txt
.rim-backups/
  node_modules-delta-<timestamp>/
    metadata.json
    summary.txt
    changed/node_modules/...
    added/node_modules/...
    binary/node_modules/...
    deleted.json
    symlinks.json
```

The comparison is not a proof of manual edits. Lifecycle scripts, native packages, generated files, and lockfile-free projects can create differences that are not hand patches. It is still much safer than blindly moving a hand-modified `node_modules` into tmpfs.

`rim adopt --diff-backup --dry-run` stays strict: it does not create scratch directories or backups. Use `rim scan --diff <project>` when you want to run the fresh-install comparison without adopting. Both `adopt --diff-backup` and `scan --diff` clean their scratch directories on success and failure.

Restore delta files later with:

```bash
rim backup list
rim backup show latest
rim backup restore latest --dry-run
rim backup restore latest
```

`rim backup show` prints metadata and a restore hint:

```txt
backup: node_modules-delta-20260708-143000
path: /home/poteto/code/app/.rim-backups/node_modules-delta-20260708-143000
project: /home/poteto/code/app
manager: bun
created_at: 1783...
manifest_hash: ...
differences:
  changed: 2
  added: 1
  deleted: 0
  type_changed: 0
  binary: 1

restore:
  rim ensure
  rim backup restore node_modules-delta-20260708-143000
```

Restore applies `changed/`, `added/`, and `binary/` files. `deleted.json` is shown but not applied unless `--apply-deletes` is passed.

If an adopted tmpfs layer disappeared after reboot, regenerate the pristine dependencies first, then restore the saved delta:

```bash
rim ensure
rim backup restore latest
```

Use deletes only when you really want to remove files that existed in the fresh baseline:

```bash
rim backup restore latest --apply-deletes
```

`.rim-backups/` can contain local patches or generated files from dependencies. It is ignored by this repo's `.gitignore` and should not be committed unless you intentionally want to publish those files.

`rim scan --diff <project>` can run the fresh-install comparison without adopting. For now it requires exactly one unmanaged candidate. With `--json`, stdout is a single JSON object with a `candidates` array plus `manual_diff` and `diff` fields when diffing is enabled. `rim scan --diff` may temporarily use as much space as a fresh install in `RIM_BASE`; for large projects, prefer `RIM_PROFILE=cache` or `RIM_PROFILE=external`.

## Layer inventory and garbage collection

`rim ls` shows every dependency layer under the active `RIM_BASE`:

```txt
PROJECT                              MANAGER    MODE           SIZE      AGE  LAST_USED PIN    ACTIVE  VERSION   LAYER
~/code/tiny-hono                     bun        tmpfs       18.3 MB      12m        1m no     no      0.1.0     /dev/shm/rim/tiny-hono-a1b2c3d4
~/code/test-vite                     npm        tmpfs       92.1 MB       2h        1h yes    pid:42  0.1.0     /dev/shm/rim/test-vite-b5c6d7e8
```

`rim pin` protects the current layer from garbage collection. `rim unpin` removes that protection.

```bash
rim pin
rim unpin
```

`rim gc` removes dependency layers by metadata. Pinned layers are skipped unless `--include-pinned` is passed. Active layers are skipped unless `--force` is passed:

```bash
rim gc --dry-run --orphaned   # show layers whose source project no longer exists
rim gc --orphaned             # remove orphaned layers
rim gc --dry-run --older-than 1d
rim gc --older-than 1d
rim gc --dry-run --all
rim gc --dry-run --all --include-pinned
rim gc --all --force            # override active-lock protection
```

With no selector, `rim gc` is intentionally safe and only previews orphaned layers. Stale active locks show as `stale:<pid>` in `rim ls`; live locks show as `pid:<pid>`.

## Doctor

`rim doctor` prints the current project's storage and risk profile without running an install:

```txt
project: /home/poteto/code/app
rim_base: /dev/shm/rim
rim_dir: /dev/shm/rim/app-<hash>
mode: tmpfs

storage:
  project:
    available: 27.8 GB
  rim_base:
    fs: tmpfs
    available: 3.7 GB

memory:
  total: 16.0 GB
  available: 9.4 GB
  shmem_used: 420.0 MB

rim:
  current_project_usage: 128.0 MB
  total_base_usage: 1.1 GB

risk:
  install_risk: low
  workspace: not detected
  lifecycle_scripts: detected
```

Install-like commands also warn when `RIM_BASE` has low available space.

## Suggestions and explain mode

`rim doctor --suggest` prints normal doctor output and then adds practical next steps:

```txt
suggestions:
  - heavy packages detected: next
    tmpfs mode may be too large; consider $HOME/.cache/rim or external storage.
  - workspace detected; shadow installs may need extra care.
  - lifecycle scripts detected; postinstall/prepare hooks may assume real project cwd.
```

`rim explain` is a dry educational view of a wrapped command. It accepts either explicit tools or shortcuts such as `rim explain install`:

```txt
tool: bun
args: install
install_like: true
cwd: /dev/shm/rim/app-<hash>/project

rim will:
  1. prepare dependency-layer layout and metadata
  2. sync manifests to the shadow project
  3. run `bun install` in the shadow project
  4. copy mutable manifests and lockfiles back on success
  5. trim bun cache unless --keep-cache is set
```

## Measured impact

`rim` is not a compression tool. It moves dependency weight away from
persistent project/cache directories and into the RAM-backed `RIM_BASE` layer.
That means RAM usage can be roughly the same as, or slightly larger than, a
normal install while the layer is alive.

The useful number is persistent disk left behind by the project.

Run the default benchmark suite with:

```bash
python3 scripts/bench.py
```

Reproduce the checked-in results, including the heavier Next.js case, with:

```bash
python3 scripts/bench.py --include-heavy
```

Install benchmarks use lifecycle-script-disabling flags where supported:

```txt
npm:  npm install --ignore-scripts --no-audit --no-fund
bun:  bun install --ignore-scripts
deno: deno cache main.ts  # imports the same dependencies through npm:<pkg>@<version> specifiers
```

Lifecycle scripts are disabled where supported: the benchmark measures dependency/cache placement, not postinstall downloads such as browser binaries or native build hooks. The same dependency sets are used across npm, bun, and deno. Deno is measured as cache placement because it has no `node_modules` install step by default. pnpm is excluded from checked-in results and is available only as an experimental opt-in manager.

Environment for the latest checked-in run:

```txt
OS: Linux
Node: v22.22.0
npm: 10.9.4
bun: 1.3.14
deno: deno 2.9.1 (stable, release, x86_64-unknown-linux-gnu)
rim: target/release/rim
Measurement: tree walk using lstat, so symlink targets are not counted as project disk usage
```

| Case | Dependencies | Normal persistent | rim persistent | rim RAM | RAM vs normal | RAM overhead | Saved persistent | Time normal | Time rim |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| npm/tiny-validation | `is-number`, `zod` | 7.6 MB | 5.2 KB | 4.8 MB | 63.53% | -2.8 MB | 99.93% | 1.197s | 1.329s |
| npm/utility-client | `axios`, `dayjs`, `lodash` | 9.8 MB | 14.9 KB | 6.3 MB | 64.07% | -3.5 MB | 99.85% | 2.255s | 2.429s |
| npm/hono-api | `@hono/node-server`, `hono`, `zod` | 16.9 MB | 5.6 KB | 7.1 MB | 41.80% | -9.9 MB | 99.97% | 1.833s | 1.952s |
| npm/react-vite-ts | `@vitejs/plugin-react`, `react`, `react-dom`, `typescript`, `vite` | 198.6 MB | 32.8 KB | 63.8 MB | 32.12% | -134.8 MB | 99.98% | 10.085s | 8.819s |
| npm/next-app | `next`, `react`, `react-dom`, `typescript` | 549.7 MB | 34.0 KB | 324.6 MB | 59.06% | -225.0 MB | 99.99% | 14.546s | 14.669s |
| bun/tiny-validation | `is-number`, `zod` | 7.4 MB | 4.7 KB | 3.5 MB | 46.50% | -4.0 MB | 99.94% | 0.263s | 0.218s |
| bun/utility-client | `axios`, `dayjs`, `lodash` | 11.0 MB | 9.4 KB | 4.9 MB | 44.16% | -6.1 MB | 99.92% | 0.545s | 0.330s |
| bun/hono-api | `@hono/node-server`, `hono`, `zod` | 13.7 MB | 5.0 KB | 5.7 MB | 41.42% | -8.0 MB | 99.96% | 0.277s | 0.241s |
| bun/react-vite-ts | `@vitejs/plugin-react`, `react`, `react-dom`, `typescript`, `vite` | 199.7 MB | 17.0 KB | 90.3 MB | 45.22% | -109.4 MB | 99.99% | 1.684s | 1.627s |
| bun/next-app | `next`, `react`, `react-dom`, `typescript` | 953.4 MB | 17.5 KB | 463.9 MB | 48.65% | -489.5 MB | 100.00% | 6.906s | 6.969s |
| deno/tiny-validation | `is-number`, `zod` | 4.8 MB | 4.6 KB | 4.6 MB | 94.59% | -266.7 KB | 99.91% | 0.601s | 0.780s |
| deno/utility-client | `axios`, `dayjs`, `lodash` | 5.9 MB | 9.2 KB | 5.2 MB | 88.05% | -727.2 KB | 99.85% | 0.617s | 0.529s |
| deno/hono-api | `@hono/node-server`, `hono`, `zod` | 10.9 MB | 4.9 KB | 9.8 MB | 89.93% | -1.1 MB | 99.96% | 0.724s | 0.574s |
| deno/react-vite-ts | `@vitejs/plugin-react`, `react`, `react-dom`, `typescript`, `vite` | 108.0 MB | 16.3 KB | 107.3 MB | 99.36% | -708.9 KB | 99.99% | 3.858s | 4.072s |
| deno/next-app | `next`, `react`, `react-dom`, `typescript` | 498.4 MB | 17.1 KB | 494.7 MB | 99.26% | -3.7 MB | 100.00% | 8.927s | 8.672s |

Latest benchmark summary output:

```txt
npm/tiny-validation: persistent 7.6 MB -> 5.2 KB, rim RAM 4.8 MB (63.53% of normal, overhead -2.8 MB), saved 99.93%
npm/utility-client: persistent 9.8 MB -> 14.9 KB, rim RAM 6.3 MB (64.07% of normal, overhead -3.5 MB), saved 99.85%
npm/hono-api: persistent 16.9 MB -> 5.6 KB, rim RAM 7.1 MB (41.80% of normal, overhead -9.9 MB), saved 99.97%
npm/react-vite-ts: persistent 198.6 MB -> 32.8 KB, rim RAM 63.8 MB (32.12% of normal, overhead -134.8 MB), saved 99.98%
npm/next-app: persistent 549.7 MB -> 34.0 KB, rim RAM 324.6 MB (59.06% of normal, overhead -225.0 MB), saved 99.99%
bun/tiny-validation: persistent 7.4 MB -> 4.7 KB, rim RAM 3.5 MB (46.50% of normal, overhead -4.0 MB), saved 99.94%
bun/utility-client: persistent 11.0 MB -> 9.4 KB, rim RAM 4.9 MB (44.16% of normal, overhead -6.1 MB), saved 99.92%
bun/hono-api: persistent 13.7 MB -> 5.0 KB, rim RAM 5.7 MB (41.42% of normal, overhead -8.0 MB), saved 99.96%
bun/react-vite-ts: persistent 199.7 MB -> 17.0 KB, rim RAM 90.3 MB (45.22% of normal, overhead -109.4 MB), saved 99.99%
bun/next-app: persistent 953.4 MB -> 17.5 KB, rim RAM 463.9 MB (48.65% of normal, overhead -489.5 MB), saved 100.00%
deno/tiny-validation: persistent 4.8 MB -> 4.6 KB, rim RAM 4.6 MB (94.59% of normal, overhead -266.7 KB), saved 99.91%
deno/utility-client: persistent 5.9 MB -> 9.2 KB, rim RAM 5.2 MB (88.05% of normal, overhead -727.2 KB), saved 99.85%
deno/hono-api: persistent 10.9 MB -> 4.9 KB, rim RAM 9.8 MB (89.93% of normal, overhead -1.1 MB), saved 99.96%
deno/react-vite-ts: persistent 108.0 MB -> 16.3 KB, rim RAM 107.3 MB (99.36% of normal, overhead -708.9 KB), saved 99.99%
deno/next-app: persistent 498.4 MB -> 17.1 KB, rim RAM 494.7 MB (99.26% of normal, overhead -3.7 MB), saved 100.00%
```

Takeaway:

- Persistent project/cache footprint drops by about 99.91-100.00% in these npm/bun/deno runs.
- npm and bun caches are trimmed after successful installs, so tiny npm/bun cases now use less RAM than the normal persistent footprint in this setup.
- Bun and Deno are especially clean in this run; bun also becomes the fastest install path here.
- pnpm is excluded from the main checked-in benchmark because its store model had high RAM overhead under tmpfs; it remains available only as an experimental opt-in benchmark target.
- Rebooting, `rim clean`, `--auto-clean`, `--ephemeral`, or deleting `RIM_BASE`
  removes the dependency layer.

Full machine-readable results are in `bench-results.json`. `scripts/bench.py` is the main benchmark entrypoint; `scripts/bench_npm.py` remains as a compatibility wrapper for the old npm-only entrypoint.

Use `--include-heavy` only when `/dev/shm` has enough free space. Use `--managers npm,bun,deno` to choose tools; `--managers pnpm` is available only for experimental comparison.

## Tests

```bash
cargo fmt --check
cargo test
cargo clippy -- -D warnings
```

Current suite:

- prepares `node_modules` symlink into RAM base
- refuses to overwrite a real `node_modules` directory
- help lists cleanup, inventory, path, explain, shortcut, and suggestion options
- dry-run reports command, `RIM_BASE`, cleanup flags, and cache envs
- `rim doctor` reports storage/memory risk and project warning signals
- status/doctor usage counts symlinks without following external targets
- `--auto-clean` removes the current dependency layer after wrapped commands while preserving exit codes
- `--ephemeral` auto-installs missing dependencies for run/test/start commands and cleans afterward
- install-like commands warn when `RIM_BASE` is low on space
- install-like commands with `--auto-clean` warn that dependencies will be removed while manifest changes remain
- `rim scan` detects unmanaged `node_modules` candidates and classifies risk/action
- `rim scan --json` emits a script-readable JSON object with a `candidates` array
- `rim scan --json --diff` keeps diff result data in stdout JSON and avoids stderr noise
- `rim scan --diff` detects manual differences without adopting
- rim-managed `node_modules` symlinks are reported as managed/skip candidates
- `rim adopt --dry-run` makes no project changes
- `rim adopt` moves a real `node_modules` into a rim layer and leaves a symlink
- `rim adopt` rolls back the move if symlink/metadata creation fails, or prints manual recovery paths
- high-risk adopt is refused unless `--allow-risk` is passed
- `rim adopt --diff-backup` saves changed/added/binary deltas under `.rim-backups`
- `rim adopt --copy` copies dependencies into the layer and preserves the original under `.rim-backups/node_modules-original-*`
- `rim adopt --diff-backup` and `rim scan --diff` clean scratch dirs on success and failure
- `rim adopt --diff-backup --dry-run` creates no scratch dirs, backups, or project changes
- `rim adopt --copy --dry-run` creates no backups, layer files, or symlinks
- `rim backup list/show/restore` manages delta backups
- `.rim-backups/` and `.rim-active` are ignored by git
- backup restore dry-runs without writing and restores changed/added/binary files by default
- `rim backup show` prints metadata, diff counts, and a restore hint
- `rim backup restore --apply-deletes` is required before deleted entries are removed
- `.rim-meta.json` powers `rim ls` and metadata-based `rim gc`
- `.rim-meta.json` stores manifest hash and pinned state
- `.rim-active` marks layers currently running wrapped commands
- `rim ls` shows ACTIVE as `no`, `pid:<pid>`, or `stale:<pid>`
- `rim repair --stale-locks` / `rim repair --broken-links` removes stale `.rim-active` files and leaves live locks alone
- `rim pin` / `rim unpin` toggle GC protection
- `rim gc --dry-run --orphaned` previews orphaned layer cleanup
- `rim gc --orphaned` removes orphaned layers
- pinned layers are skipped by GC unless `--include-pinned` is used
- active layers are skipped by GC unless `--force` is used
- `rim clean` refuses active layers unless `--force` is used
- `rim path` prints script-friendly dependency-layer paths
- `rim manager` reports detected manager and reason
- `rim install` / `rim run dev` auto-detect Bun or npm from project files
- `rim ensure` installs missing/stale dependencies and skips when dependencies already exist and manifest hash matches
- shortcut `rim run` / `rim test` / `rim start` ensure dependencies before running
- `RIM_PROFILE=cache` maps to `$HOME/.cache/rim` when `RIM_BASE` is unset
- Deno commands do not create a project `node_modules` symlink
- `.rim-meta.json` is updated with an atomic write+rename
- `rim explain` reports the command plan without changing files
- `rim doctor --suggest` reports practical storage/project suggestions
- `rim clean --cache-only` and `rim clean --deps-only` remove only one side of the layer
- `bunfig.toml` is synced to the shadow project
- npm/bun install-like commands trim package-manager cache by default; `--keep-cache` preserves it
- pnpm is experimental/opt-in and injects `--store-dir` when used
- install-like commands run in RAM shadow project and copy lockfiles back
- clean removes only the current project's RAM dir and symlink

## Caveats

- Linux-first and WSL-friendly. Native Windows builds are compile-only and intentionally refuse to run. `/dev/shm` is the Linux/WSL default because the main target is small Linux boxes and disposable dependency state.
- RAM/tmpfs is finite; large Playwright/Next/Expo/Electron installs can still blow up `/dev/shm`.
- Do not store hand-edited `node_modules` only in tmpfs. If `RIM_BASE` is `/dev/shm/rim`, adopted dependencies disappear on reboot; use `--diff-backup`, `--copy`, `RIM_PROFILE=cache`, or `RIM_PROFILE=external`.
- Active-lock protection is best-effort and based on `/proc/<pid>` on Linux/WSL; PID reuse is theoretically possible.
- `--force` overrides active-lock protection. Use it only for confirmed stale locks or emergency cleanup.
- Keeping the repo public is fine for rim itself, but never publish `.rim-backups/` unless you have reviewed the dependency diffs inside.
- Restarting clears `/dev/shm`; run `rim repair --broken-links` if the project link is broken, then `rim ensure` to recreate dependencies.
- For long-lived or heavy projects on tiny machines, consider `RIM_BASE=$HOME/.cache/rim` or an external drive.
- `--ephemeral` auto-installs only for package-manager `run`, `test`, and `start` commands for now.
- Manager shortcuts are intentionally simple: packageManager wins first, then Bun lockfiles, then npm, then pnpm experimental.
- pnpm is not recommended for RAM/tmpfs mode; use npm or bun for the main path, or move `RIM_BASE` to cache/external storage if experimenting with pnpm.
- Some package-manager edge cases are not handled yet, especially workspaces and lifecycle scripts that assume install cwd is the real source tree.
- This is intentionally not a global package manager replacement. It is a small wrapper for ephemeral or isolated dependencies.
