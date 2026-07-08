#!/usr/bin/env python3
"""Compatibility wrapper for the old npm-only benchmark entrypoint."""

from __future__ import annotations

import runpy
import sys
from pathlib import Path


if not any(arg == "--managers" or arg.startswith("--managers=") for arg in sys.argv[1:]):
    sys.argv.extend(["--managers", "npm"])

runpy.run_path(str(Path(__file__).with_name("bench.py")), run_name="__main__")
