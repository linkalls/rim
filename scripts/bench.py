#!/usr/bin/env python3
"""Benchmark rim dependency/cache placement across package managers.

npm/pnpm/bun benchmarks measure install dependency placement. They intentionally
use lifecycle-script-disabling flags where the package manager supports them, so
postinstall scripts do not download browser binaries, native toolchains, or other
surprise payloads.

Deno is different: it has no node_modules install step by default, so the Deno
case measures cache placement with `deno cache main.ts`.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_WORKDIR = Path(os.environ.get("RIM_BENCH_WORKDIR", "/tmp/rim-bench"))
DEFAULT_RIM_BASE = Path(os.environ.get("RIM_BENCH_RIM_BASE", "/dev/shm/rim-bench"))


@dataclass(frozen=True)
class PackageSet:
    name: str
    description: str
    dependencies: dict[str, str]


@dataclass(frozen=True)
class Manager:
    name: str
    command: str
    kind: str


NODE_MANAGERS = [
    Manager(name="npm", command="npm", kind="node-install"),
    Manager(name="pnpm", command="pnpm", kind="node-install"),
    Manager(name="bun", command="bun", kind="node-install"),
]
DENO_MANAGER = Manager(name="deno", command="deno", kind="deno-cache")

PACKAGE_SETS = [
    PackageSet(
        name="tiny-validation",
        description="tiny + validator dependency pair",
        dependencies={"is-number": "7.0.0", "zod": "3.25.76"},
    ),
    PackageSet(
        name="utility-client",
        description="common utility/client libraries",
        dependencies={"axios": "1.13.2", "dayjs": "1.11.19", "lodash": "4.17.21"},
    ),
    PackageSet(
        name="hono-api",
        description="small API stack",
        dependencies={"@hono/node-server": "1.19.6", "hono": "4.12.9", "zod": "4.3.6"},
    ),
    PackageSet(
        name="react-vite-ts",
        description="small frontend stack",
        dependencies={
            "@vitejs/plugin-react": "6.0.3",
            "react": "19.2.7",
            "react-dom": "19.2.7",
            "typescript": "6.0.3",
            "vite": "8.1.3",
        },
    ),
]

HEAVY_PACKAGE_SETS = [
    PackageSet(
        name="next-app",
        description="heavier React framework stack",
        dependencies={"next": "16.2.10", "react": "19.2.7", "react-dom": "19.2.7", "typescript": "6.0.3"},
    ),
]

DENO_CASES = [
    PackageSet(
        name="deno-zod-cache",
        description="Deno npm: zod cache placement",
        dependencies={"zod": "3.25.76"},
    )
]


def command_exists(command: str) -> bool:
    return shutil.which(command) is not None


def command_version(command: str) -> str | None:
    if not command_exists(command):
        return None
    completed = subprocess.run(
        [command, "--version"],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    if completed.returncode != 0:
        return None
    return completed.stdout.strip().splitlines()[0] if completed.stdout.strip() else None


def run(command: list[str], cwd: Path, env: dict[str, str] | None = None) -> float:
    started = time.perf_counter()
    completed = subprocess.run(
        command,
        cwd=cwd,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    elapsed = time.perf_counter() - started
    if completed.returncode != 0:
        print(completed.stdout, file=sys.stderr)
        raise RuntimeError(f"command failed ({completed.returncode}): {' '.join(command)}")
    return elapsed


def lstat_tree(path: Path) -> int:
    if not path.exists() and not path.is_symlink():
        return 0
    total = 0
    stack = [path]
    while stack:
        current = stack.pop()
        try:
            stat = current.lstat()
        except FileNotFoundError:
            continue
        total += stat.st_size
        if current.is_dir() and not current.is_symlink():
            try:
                stack.extend(current.iterdir())
            except PermissionError:
                continue
    return total


def allocated_tree(path: Path) -> int:
    if not path.exists() and not path.is_symlink():
        return 0
    total = 0
    stack = [path]
    while stack:
        current = stack.pop()
        try:
            stat = current.lstat()
        except FileNotFoundError:
            continue
        total += stat.st_blocks * 512
        if current.is_dir() and not current.is_symlink():
            try:
                stack.extend(current.iterdir())
            except PermissionError:
                continue
    return total


def write_package_json(project: Path, package_set: PackageSet) -> None:
    package_json = {
        "name": f"rim-bench-{package_set.name}",
        "version": "0.0.0",
        "private": True,
        "dependencies": package_set.dependencies,
    }
    project.mkdir(parents=True, exist_ok=True)
    (project / "package.json").write_text(json.dumps(package_json, indent=2) + "\n")


def write_deno_project(project: Path, package_set: PackageSet) -> None:
    project.mkdir(parents=True, exist_ok=True)
    deps = []
    for name, version in package_set.dependencies.items():
        ident = name.replace("@", "").replace("/", "_").replace("-", "_")
        deps.append(f'import * as {ident} from "npm:{name}@{version}";')
    body = "\n".join(deps) + "\nconsole.log('rim deno cache benchmark');\n"
    (project / "main.ts").write_text(body)
    (project / "deno.json").write_text(json.dumps({"nodeModulesDir": "none"}, indent=2) + "\n")


def remove_path(path: Path) -> None:
    if path.is_symlink() or path.is_file():
        path.unlink(missing_ok=True)
    elif path.exists():
        shutil.rmtree(path)


def rim_project_dir(rim_base: Path, project: Path, tool: str, args: list[str]) -> Path | None:
    completed = subprocess.run(
        [str(REPO_ROOT / "target" / "release" / "rim"), "--dry-run", tool, *args],
        cwd=project,
        env=base_env({"RIM_BASE": str(rim_base)}),
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    for line in completed.stdout.splitlines():
        if line.startswith("rim_dir: "):
            return Path(line.removeprefix("rim_dir: ").strip())
    return None


def manager_install_args(manager: Manager, cache_dir: Path | None = None) -> list[str]:
    if manager.name == "npm":
        return ["install", "--ignore-scripts", "--no-audit", "--no-fund"]
    if manager.name == "pnpm":
        args = []
        if cache_dir is not None:
            args.extend(["--store-dir", str(cache_dir)])
        args.extend(["install", "--ignore-scripts"])
        return args
    if manager.name == "bun":
        return ["install", "--ignore-scripts"]
    raise ValueError(f"unsupported install manager: {manager.name}")


def base_env(extra: dict[str, str] | None = None) -> dict[str, str]:
    env = {**os.environ, "COREPACK_ENABLE_DOWNLOAD_PROMPT": "0"}
    if extra:
        env.update(extra)
    return env


def normal_env_for(manager: Manager, cache_dir: Path) -> dict[str, str]:
    env = base_env()
    if manager.name == "npm":
        env["npm_config_cache"] = str(cache_dir)
    elif manager.name == "bun":
        env["BUN_INSTALL_CACHE_DIR"] = str(cache_dir)
    return env


def bench_node_manager(
    manager: Manager,
    package_set: PackageSet,
    workdir: Path,
    rim_base: Path,
    keep: bool,
) -> dict[str, Any]:
    case_root = workdir / manager.name / package_set.name
    conventional_project = case_root / "normal-project"
    conventional_cache = case_root / f"normal-{manager.name}-cache"
    rim_project = case_root / "rim-project"
    case_rim_base = rim_base / manager.name / package_set.name

    remove_path(case_root)
    remove_path(case_rim_base)
    conventional_project.mkdir(parents=True)
    rim_project.mkdir(parents=True)
    conventional_cache.mkdir(parents=True)
    case_rim_base.mkdir(parents=True)

    write_package_json(conventional_project, package_set)
    write_package_json(rim_project, package_set)

    normal_args = manager_install_args(manager, conventional_cache if manager.name == "pnpm" else None)
    rim_args = manager_install_args(manager)

    normal_seconds = run([manager.command, *normal_args], conventional_project, normal_env_for(manager, conventional_cache))
    rim_seconds = run([str(REPO_ROOT / "target" / "release" / "rim"), manager.command, *rim_args], rim_project, base_env({"RIM_BASE": str(case_rim_base)}))

    node_modules_link = rim_project / "node_modules"
    link_target = os.readlink(node_modules_link) if node_modules_link.is_symlink() else None
    rim_dir = rim_project_dir(case_rim_base, rim_project, manager.command, rim_args)

    normal_project_lstat = lstat_tree(conventional_project)
    normal_cache_lstat = lstat_tree(conventional_cache)
    rim_project_lstat = lstat_tree(rim_project)
    rim_ram_lstat = lstat_tree(case_rim_base)

    result = {
        "manager": manager.name,
        "kind": manager.kind,
        "name": package_set.name,
        "description": package_set.description,
        "dependencies": package_set.dependencies,
        "normal_command": [manager.command, *normal_args],
        "rim_command": ["rim", manager.command, *rim_args],
        "normal": {
            "seconds": round(normal_seconds, 3),
            "project_lstat_bytes": normal_project_lstat,
            "project_allocated_bytes": allocated_tree(conventional_project),
            "node_modules_lstat_bytes": lstat_tree(conventional_project / "node_modules"),
            "cache_lstat_bytes": normal_cache_lstat,
            "total_persistent_lstat_bytes": normal_project_lstat + normal_cache_lstat,
        },
        "rim": {
            "seconds": round(rim_seconds, 3),
            "project_lstat_bytes": rim_project_lstat,
            "project_allocated_bytes": allocated_tree(rim_project),
            "node_modules_is_symlink": node_modules_link.is_symlink(),
            "node_modules_link_target": link_target,
            "rim_dir": str(rim_dir) if rim_dir else None,
            "ram_lstat_bytes": rim_ram_lstat,
            "total_persistent_lstat_bytes": rim_project_lstat,
        },
    }
    add_savings(result)

    if not keep:
        remove_path(case_root)
        remove_path(case_rim_base)

    return result


def bench_deno_cache(package_set: PackageSet, workdir: Path, rim_base: Path, keep: bool) -> dict[str, Any]:
    manager = DENO_MANAGER
    case_root = workdir / manager.name / package_set.name
    conventional_project = case_root / "normal-project"
    conventional_cache = case_root / "normal-deno-cache"
    rim_project = case_root / "rim-project"
    case_rim_base = rim_base / manager.name / package_set.name

    remove_path(case_root)
    remove_path(case_rim_base)
    conventional_project.mkdir(parents=True)
    rim_project.mkdir(parents=True)
    conventional_cache.mkdir(parents=True)
    case_rim_base.mkdir(parents=True)

    write_deno_project(conventional_project, package_set)
    write_deno_project(rim_project, package_set)

    normal_args = ["cache", "main.ts"]
    normal_env = base_env({"DENO_DIR": str(conventional_cache)})
    rim_args = ["cache", "main.ts"]
    rim_env = base_env({"RIM_BASE": str(case_rim_base)})

    normal_seconds = run([manager.command, *normal_args], conventional_project, normal_env)
    rim_seconds = run([str(REPO_ROOT / "target" / "release" / "rim"), manager.command, *rim_args], rim_project, rim_env)

    node_modules_link = rim_project / "node_modules"
    rim_dir = rim_project_dir(case_rim_base, rim_project, manager.command, rim_args)
    normal_project_lstat = lstat_tree(conventional_project)
    normal_cache_lstat = lstat_tree(conventional_cache)
    rim_project_lstat = lstat_tree(rim_project)
    rim_ram_lstat = lstat_tree(case_rim_base)

    result = {
        "manager": manager.name,
        "kind": manager.kind,
        "name": package_set.name,
        "description": package_set.description,
        "dependencies": package_set.dependencies,
        "normal_command": [manager.command, *normal_args],
        "rim_command": ["rim", manager.command, *rim_args],
        "normal": {
            "seconds": round(normal_seconds, 3),
            "project_lstat_bytes": normal_project_lstat,
            "project_allocated_bytes": allocated_tree(conventional_project),
            "cache_lstat_bytes": normal_cache_lstat,
            "total_persistent_lstat_bytes": normal_project_lstat + normal_cache_lstat,
        },
        "rim": {
            "seconds": round(rim_seconds, 3),
            "project_lstat_bytes": rim_project_lstat,
            "project_allocated_bytes": allocated_tree(rim_project),
            "node_modules_is_symlink": node_modules_link.is_symlink(),
            "node_modules_link_target": os.readlink(node_modules_link) if node_modules_link.is_symlink() else None,
            "rim_dir": str(rim_dir) if rim_dir else None,
            "ram_lstat_bytes": rim_ram_lstat,
            "total_persistent_lstat_bytes": rim_project_lstat,
        },
    }
    add_savings(result)

    if not keep:
        remove_path(case_root)
        remove_path(case_rim_base)

    return result


def add_savings(result: dict[str, Any]) -> None:
    saved = result["normal"]["total_persistent_lstat_bytes"] - result["rim"]["total_persistent_lstat_bytes"]
    normal_total = result["normal"]["total_persistent_lstat_bytes"]
    ram = result["rim"]["ram_lstat_bytes"]
    result["saved_persistent_lstat_bytes"] = saved
    result["saved_persistent_percent"] = round((saved / normal_total) * 100, 2) if normal_total else 0
    result["ram_vs_normal_persistent_percent"] = round((ram / normal_total) * 100, 2) if normal_total else 0
    result["rim_ram_overhead_lstat_bytes"] = ram - normal_total


def format_bytes(value: int) -> str:
    units = ["B", "KB", "MB", "GB"]
    sign = "-" if value < 0 else ""
    amount = float(abs(value))
    for unit in units:
        if amount < 1024 or unit == units[-1]:
            if unit == "B":
                return f"{sign}{int(amount)} B"
            return f"{sign}{amount:.1f} {unit}"
        amount /= 1024
    return f"{value} B"


def result_summary(result: dict[str, Any]) -> str:
    normal = result["normal"]["total_persistent_lstat_bytes"]
    persistent = result["rim"]["total_persistent_lstat_bytes"]
    ram = result["rim"]["ram_lstat_bytes"]
    overhead = result["rim_ram_overhead_lstat_bytes"]
    overhead_prefix = "+" if overhead >= 0 else ""
    return (
        f"{result['manager']}/{result['name']}: persistent {format_bytes(normal)} -> {format_bytes(persistent)}, "
        f"rim RAM {format_bytes(ram)} "
        f"({result['ram_vs_normal_persistent_percent']:.2f}% of normal, "
        f"overhead {overhead_prefix}{format_bytes(overhead)}), "
        f"saved {result['saved_persistent_percent']:.2f}%"
    )


def parse_managers(value: str) -> list[str]:
    if value == "all":
        return [m.name for m in NODE_MANAGERS] + [DENO_MANAGER.name]
    return [part.strip() for part in value.split(",") if part.strip()]


def main() -> int:
    parser = argparse.ArgumentParser(description="Benchmark rim dependency/cache placement.")
    parser.add_argument("--output", type=Path, default=REPO_ROOT / "bench-results.json")
    parser.add_argument("--workdir", type=Path, default=DEFAULT_WORKDIR)
    parser.add_argument("--rim-base", type=Path, default=DEFAULT_RIM_BASE)
    parser.add_argument("--include-heavy", action="store_true")
    parser.add_argument("--managers", default="npm,pnpm,bun,deno", help="comma-separated managers or all")
    parser.add_argument("--keep", action="store_true", help="keep temporary benchmark projects")
    args = parser.parse_args()

    rim_bin = REPO_ROOT / "target" / "release" / "rim"
    if not rim_bin.exists():
        run([str(Path.home() / ".cargo" / "bin" / "cargo"), "build", "--release", "--quiet"], REPO_ROOT)

    package_sets = PACKAGE_SETS + (HEAVY_PACKAGE_SETS if args.include_heavy else [])
    requested = parse_managers(args.managers)
    manager_map = {m.name: m for m in NODE_MANAGERS + [DENO_MANAGER]}
    results: list[dict[str, Any]] = []
    skipped: list[dict[str, str]] = []

    for name in requested:
        if name not in manager_map:
            raise SystemExit(f"unknown manager: {name}")
        manager = manager_map[name]
        if not command_exists(manager.command):
            print(f"== {manager.name}: skipped, command not found ==", flush=True)
            skipped.append({"manager": manager.name, "reason": f"{manager.command} not found"})
            continue
        if manager.kind == "node-install":
            for package_set in package_sets:
                print(f"== {manager.name}/{package_set.name} ==", flush=True)
                try:
                    results.append(bench_node_manager(manager, package_set, args.workdir, args.rim_base, args.keep))
                except RuntimeError as err:
                    print(f"!! skipped {manager.name}/{package_set.name}: {err}", file=sys.stderr, flush=True)
                    skipped.append({"manager": manager.name, "case": package_set.name, "reason": str(err)})
        else:
            for package_set in DENO_CASES:
                print(f"== {manager.name}/{package_set.name} ==", flush=True)
                try:
                    results.append(bench_deno_cache(package_set, args.workdir, args.rim_base, args.keep))
                except RuntimeError as err:
                    print(f"!! skipped {manager.name}/{package_set.name}: {err}", file=sys.stderr, flush=True)
                    skipped.append({"manager": manager.name, "case": package_set.name, "reason": str(err)})

    payload = {
        "tool": "rim",
        "benchmark": "dependency/cache placement",
        "measurement": "lstat_tree does not follow symlinks; allocated bytes are also recorded for project trees",
        "manager_versions": {name: command_version(m.command) for name, m in manager_map.items()},
        "requested_managers": requested,
        "skipped": skipped,
        "notes": [
            "npm/pnpm/bun install benchmarks disable lifecycle scripts where supported to avoid download/build side effects.",
            "Deno has no node_modules install step by default, so its case measures DENO_DIR cache placement with deno cache.",
            "rim is expected to reduce persistent disk usage, not total bytes used while the dependency layer is alive.",
            "The RAM layer is tmpfs-backed by default and disappears on reboot, rim clean, --auto-clean, or --ephemeral cleanup.",
        ],
        "results": results,
    }
    args.output.write_text(json.dumps(payload, indent=2) + "\n")

    print("\nSummary")
    for result in results:
        print(result_summary(result))
    if skipped:
        print("\nSkipped")
        for item in skipped:
            print(f"{item['manager']}: {item['reason']}")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
