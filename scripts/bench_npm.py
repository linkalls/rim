#!/usr/bin/env python3
"""Benchmark rim against conventional npm installs.

The benchmark intentionally runs npm with --ignore-scripts so package lifecycle
scripts do not download browser binaries, native toolchains, or other surprise
payloads. It measures dependency footprint placement, not package build hooks.
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
NPM_ARGS = ["install", "--ignore-scripts", "--no-audit", "--no-fund"]


@dataclass(frozen=True)
class PackageSet:
    name: str
    description: str
    dependencies: dict[str, str]


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
        dependencies={"@vitejs/plugin-react": "6.0.3", "react": "19.2.7", "react-dom": "19.2.7", "typescript": "6.0.3", "vite": "8.1.3"},
    ),
]


HEAVY_PACKAGE_SETS = [
    PackageSet(
        name="next-app",
        description="heavier React framework stack",
        dependencies={"next": "16.2.10", "react": "19.2.7", "react-dom": "19.2.7", "typescript": "6.0.3"},
    ),
]


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


def remove_path(path: Path) -> None:
    if path.is_symlink() or path.is_file():
        path.unlink(missing_ok=True)
    elif path.exists():
        shutil.rmtree(path)


def rim_project_dir(rim_base: Path, project: Path) -> Path | None:
    # Ask rim because the hash is intentionally internal to the CLI.
    completed = subprocess.run(
        [str(REPO_ROOT / "target" / "release" / "rim"), "--dry-run", "npm", "install"],
        cwd=project,
        env={**os.environ, "RIM_BASE": str(rim_base)},
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    for line in completed.stdout.splitlines():
        if line.startswith("rim_dir: "):
            return Path(line.removeprefix("rim_dir: ").strip())
    return None


def bench_one(package_set: PackageSet, workdir: Path, rim_base: Path, keep: bool) -> dict[str, Any]:
    case_root = workdir / package_set.name
    conventional_project = case_root / "normal-project"
    conventional_cache = case_root / "normal-npm-cache"
    rim_project = case_root / "rim-project"
    case_rim_base = rim_base / package_set.name

    remove_path(case_root)
    remove_path(case_rim_base)
    conventional_project.mkdir(parents=True)
    rim_project.mkdir(parents=True)
    conventional_cache.mkdir(parents=True)
    case_rim_base.mkdir(parents=True)

    write_package_json(conventional_project, package_set)
    write_package_json(rim_project, package_set)

    normal_env = {**os.environ, "npm_config_cache": str(conventional_cache)}
    rim_env = {**os.environ, "RIM_BASE": str(case_rim_base)}

    normal_seconds = run(["npm", *NPM_ARGS], conventional_project, normal_env)
    rim_seconds = run([str(REPO_ROOT / "target" / "release" / "rim"), "npm", *NPM_ARGS], rim_project, rim_env)

    node_modules_link = rim_project / "node_modules"
    link_target = os.readlink(node_modules_link) if node_modules_link.is_symlink() else None
    rim_dir = rim_project_dir(case_rim_base, rim_project)

    normal_project_lstat = lstat_tree(conventional_project)
    normal_cache_lstat = lstat_tree(conventional_cache)
    rim_project_lstat = lstat_tree(rim_project)
    rim_ram_lstat = lstat_tree(case_rim_base)

    result = {
        "name": package_set.name,
        "description": package_set.description,
        "dependencies": package_set.dependencies,
        "normal": {
            "seconds": round(normal_seconds, 3),
            "project_lstat_bytes": normal_project_lstat,
            "project_allocated_bytes": allocated_tree(conventional_project),
            "node_modules_lstat_bytes": lstat_tree(conventional_project / "node_modules"),
            "npm_cache_lstat_bytes": normal_cache_lstat,
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

    saved = result["normal"]["total_persistent_lstat_bytes"] - result["rim"]["total_persistent_lstat_bytes"]
    normal_total = result["normal"]["total_persistent_lstat_bytes"]
    ram = result["rim"]["ram_lstat_bytes"]
    result["saved_persistent_lstat_bytes"] = saved
    result["saved_persistent_percent"] = round((saved / normal_total) * 100, 2) if normal_total else 0
    result["ram_vs_normal_persistent_percent"] = round((ram / normal_total) * 100, 2) if normal_total else 0
    result["rim_ram_overhead_lstat_bytes"] = ram - normal_total

    if not keep:
        remove_path(case_root)
        remove_path(case_rim_base)

    return result


def format_bytes(value: int) -> str:
    units = ["B", "KB", "MB", "GB"]
    amount = float(value)
    for unit in units:
        if amount < 1024 or unit == units[-1]:
            if unit == "B":
                return f"{int(amount)} B"
            return f"{amount:.1f} {unit}"
        amount /= 1024
    return f"{value} B"


def main() -> int:
    parser = argparse.ArgumentParser(description="Benchmark rim npm dependency placement.")
    parser.add_argument("--output", type=Path, default=REPO_ROOT / "bench-results.json")
    parser.add_argument("--workdir", type=Path, default=DEFAULT_WORKDIR)
    parser.add_argument("--rim-base", type=Path, default=DEFAULT_RIM_BASE)
    parser.add_argument("--include-heavy", action="store_true")
    parser.add_argument("--keep", action="store_true", help="keep temporary benchmark projects")
    args = parser.parse_args()

    rim_bin = REPO_ROOT / "target" / "release" / "rim"
    if not rim_bin.exists():
        run([str(Path.home() / ".cargo" / "bin" / "cargo"), "build", "--release", "--quiet"], REPO_ROOT)

    package_sets = PACKAGE_SETS + (HEAVY_PACKAGE_SETS if args.include_heavy else [])
    results = []
    for package_set in package_sets:
        print(f"== {package_set.name} ==", flush=True)
        results.append(bench_one(package_set, args.workdir, args.rim_base, args.keep))

    payload = {
        "tool": "rim",
        "benchmark": "npm install dependency placement",
        "measurement": "lstat_tree does not follow symlinks; allocated bytes are also recorded for project trees",
        "npm_args": NPM_ARGS,
        "notes": [
            "npm is run with --ignore-scripts to avoid lifecycle downloads and native build side effects.",
            "rim is expected to reduce persistent disk usage, not total bytes used while the RAM layer is alive.",
            "The RAM layer is tmpfs-backed by default and disappears on reboot or rim clean.",
        ],
        "results": results,
    }
    args.output.write_text(json.dumps(payload, indent=2) + "\n")

    print("\nSummary")
    for result in results:
        normal = result["normal"]["total_persistent_lstat_bytes"]
        persistent = result["rim"]["total_persistent_lstat_bytes"]
        ram = result["rim"]["ram_lstat_bytes"]
        print(
            f"{result['name']}: persistent {format_bytes(normal)} -> {format_bytes(persistent)}, "
            f"rim RAM {format_bytes(ram)}, saved {result['saved_persistent_percent']}%"
        )

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
