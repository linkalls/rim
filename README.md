# rim

**rim** is a tiny Rust CLI for keeping project source on disk while putting JavaScript dependency weight in RAM.

The core idea:

> Keep source. Evaporate dependencies.

`rim npm install` / `rim pnpm install` / `rim bun install` prepares a RAM-backed shadow project under `/dev/shm`, installs dependencies there, and leaves your real project with only a `node_modules` symlink.

This is for small experiments and projects where `node_modules` / package-manager caches feel too expensive to leave on disk.

## Current status

MVP, Linux-first.

Implemented:

- `rim prepare`
- `rim status`
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

Remove the current project's RAM dependency directory and `node_modules` symlink:

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

`RIM_BASE` controls where RAM state is stored:

```bash
RIM_BASE=/dev/shm/rim rim npm install
```

Default:

```txt
/dev/shm/rim
```

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

## Measured impact

Environment:

```txt
OS: Linux
Node: v22.22.0
npm: 10.9.4
rim: target/release/rim
Package set: is-number@7.0.0 + zod@3.25.76
Command: npm install --ignore-scripts --no-audit --no-fund
```

Measured with a tree walk using `lstat`, so symlink targets are **not** counted as project disk usage.

| Mode | Time | Persistent project bytes | Persistent cache bytes | Persistent total | RAM bytes |
|---|---:|---:|---:|---:|---:|
| normal npm | 1.183s | 3,724,377 | 4,275,612 | 7,999,989 | 0 |
| rim npm | 1.375s | 5,141 | 0 | 5,141 | 9,265,525 |

Result:

```txt
persistent disk saved: 7,994,848 bytes
persistent disk reduction: 99.94%
extra time in this run: ~0.192s
```

Functional check after `rim npm install`:

```bash
node -e "const {z}=require('zod'); console.log(z.string().parse('ok'))"
# ok
```

So for this tiny dependency set, the persistent disk footprint went from about **8.0 MB** to about **5 KB**. The dependency mass moved to `/dev/shm` and disappears on reboot or `rim clean`.

## Tests

```bash
cargo fmt --check
cargo test
cargo clippy -- -D warnings
```

Current suite:

- prepares `node_modules` symlink into RAM base
- refuses to overwrite a real `node_modules` directory
- dry-run reports command and cache envs
- pnpm injects `--store-dir`
- install-like commands run in RAM shadow project and copy lockfiles back
- clean removes only the current project's RAM dir and symlink

## Caveats

- Linux-first. `/dev/shm` is assumed by default.
- RAM is finite; large Playwright/Next installs can still blow up `/dev/shm`.
- Restarting clears `/dev/shm`; run `rim npm install` again to recreate dependencies.
- Some package-manager edge cases are not handled yet, especially workspaces and lifecycle scripts that assume install cwd is the real source tree.
- This is intentionally not a global package manager replacement. It is a small wrapper for ephemeral dependencies.
