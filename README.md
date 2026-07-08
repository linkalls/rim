# rim

**rim** is a tiny Rust CLI for storage-starved Linux machines: keep project source on disk, but move disposable JavaScript dependency weight into a separate dependency layer.

The core idea:

> Keep source. Evaporate dependencies.

`rim npm install` / `rim pnpm install` / `rim bun install` prepares a shadow project under `RIM_BASE` (`/dev/shm/rim` by default), installs dependencies there, and leaves your real project with only a `node_modules` symlink.

This is especially useful on small experiments, AI-generated throwaway apps, and mini PCs where 32-64 GB eMMC/SSD storage is more precious than disposable dependency state.

## Current status

MVP, Linux-first.

Implemented:

- `rim prepare`
- `rim status`
- `rim doctor`
- `rim clean`
- `rim [--dry-run] npm ...`
- `rim [--dry-run] pnpm ...`
- `rim [--dry-run] bun ...`
- `rim [--dry-run] deno ...` cache env wiring
- RAM-backed npm cache / pnpm store / Deno cache / Bun cache envs
- install-like commands run in a shadow project so npm cannot replace the project symlink with a real directory
- generated lockfiles are copied back to the real project after successful install-like commands

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

For pnpm:

```bash
rim pnpm install
rim pnpm run dev
```

For Bun:

```bash
rim bun install
rim bun run dev
```

For Deno, `rim` mainly redirects caches:

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
```

Remove the current project's dependency directory and `node_modules` symlink:

```bash
rim clean
```

Dry-run without executing the tool:

```bash
rim --dry-run npm install
```

## How it works

For a project like:

```txt
/home/poteto/code/app
```

`rim` creates:

```txt
/dev/shm/rim/app-<hash>/
  project/
    package.json
    package-lock.json
    node_modules/
  npm-cache/
  pnpm-store/
  deno-cache/
  bun-cache/
  xdg-cache/
  tmp/
```

The real project gets:

```txt
node_modules -> /dev/shm/rim/app-<hash>/project/node_modules
```

For install-like commands (`install`, `i`, `add`, `remove`, `rm`, `update`, `up`, `ci`), `rim` runs the package manager inside the RAM shadow project. This matters because `npm install` may replace a pre-existing `node_modules` symlink if it is run directly in the source project.

For non-install commands (`run dev`, `test`, etc.), `rim` runs from the real project so relative source paths behave normally.

## Environment

`RIM_BASE` controls where the dependency layer is stored.

Default RAM/tmpfs mode:

```bash
RIM_BASE=/dev/shm/rim rim npm install
```

Default when unset:

```txt
/dev/shm/rim
```

You can choose different operating modes:

| Mode | Example | Tradeoff |
|---|---|---|
| RAM/tmpfs | `RIM_BASE=/dev/shm/rim` | Fast and fully disposable; uses RAM/shared memory |
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

For pnpm it also runs:

```bash
pnpm --store-dir <rim-dir>/pnpm-store ...
```

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

## Measured impact

`rim` is not a compression tool. It moves dependency weight away from
persistent project/cache directories and into the RAM-backed `RIM_BASE` layer.
That means RAM usage can be roughly the same as, or slightly larger than, a
normal install while the layer is alive.

The useful number is persistent disk left behind by the project.

Run the benchmark suite with:

```bash
python3 scripts/bench_npm.py
```

By default it benchmarks several npm dependency sets. Add `--include-heavy` to include the Next.js case used in the checked-in results.
All cases use:

```bash
npm install --ignore-scripts --no-audit --no-fund
```

`--ignore-scripts` is intentional: the benchmark measures dependency placement,
not lifecycle downloads such as browser binaries or native build hooks.

Environment for the latest checked-in run:

```txt
OS: Linux
Node: v22.22.0
npm: 10.9.4
rim: target/release/rim
Measurement: tree walk using lstat, so symlink targets are not counted as project disk usage
```

| Package set | Dependencies | Normal persistent | rim persistent | rim RAM | Saved persistent | Time normal | Time rim |
|---|---|---:|---:|---:|---:|---:|---:|
| tiny-validation | `is-number`, `zod` | 7.6 MB | 5.2 KB | 8.8 MB | 99.93% | 1.210s | 1.361s |
| utility-client | `axios`, `dayjs`, `lodash` | 9.8 MB | 14.9 KB | 10.0 MB | 99.85% | 2.173s | 2.263s |
| hono-api | `@hono/node-server`, `hono`, `zod` | 16.9 MB | 5.6 KB | 17.3 MB | 99.97% | 1.769s | 1.869s |
| react-vite-ts | `@vitejs/plugin-react`, `react`, `react-dom`, `typescript`, `vite` | 198.6 MB | 32.8 KB | 198.4 MB | 99.98% | 8.536s | 8.416s |
| next-app | `next`, `react`, `react-dom`, `typescript` | 549.7 MB | 34.0 KB | 546.6 MB | 99.99% | 14.268s | 14.403s |

Takeaway:

- Persistent project/cache footprint drops by about 99.85-99.99% in these runs.
- RAM usage is not magic: for tiny installs it can be a bit larger than normal
  persistent usage because `rim` keeps a shadow project plus RAM caches.
- For larger small-app stacks, the RAM layer is roughly the dependency mass that
  would otherwise live on disk.
- Rebooting, `rim clean`, or deleting `RIM_BASE` removes the dependency layer.

Full machine-readable results are in `bench-results.json`.

Heavy package sets such as Next.js are opt-in when rerunning the suite:

```bash
python3 scripts/bench_npm.py --include-heavy
```

Use that only when `/dev/shm` has enough free space.

## Tests

```bash
cargo fmt --check
cargo test
cargo clippy -- -D warnings
```

Current suite:

- prepares `node_modules` symlink into RAM base
- refuses to overwrite a real `node_modules` directory
- dry-run reports command, `RIM_BASE`, and cache envs
- `rim doctor` reports storage/memory risk and project warning signals
- install-like commands warn when `RIM_BASE` is low on space
- pnpm injects `--store-dir`
- install-like commands run in RAM shadow project and copy lockfiles back
- clean removes only the current project's RAM dir and symlink

## Caveats

- Linux-first. `/dev/shm` is the default because the main target is small Linux boxes and disposable dependency state.
- RAM/tmpfs is finite; large Playwright/Next/Expo/Electron installs can still blow up `/dev/shm`.
- Restarting clears `/dev/shm`; run `rim npm install` again to recreate dependencies.
- For long-lived or heavy projects on tiny machines, consider `RIM_BASE=$HOME/.cache/rim` or an external drive.
- Some package-manager edge cases are not handled yet, especially workspaces and lifecycle scripts that assume install cwd is the real source tree.
- This is intentionally not a global package manager replacement. It is a small wrapper for ephemeral or isolated dependencies.
