#!/usr/bin/env python3
"""Compatibility wrapper for the old npm-only benchmark entrypoint."""

from pathlib import Path
import runpy

runpy.run_path(str(Path(__file__).with_name("bench.py")), run_name="__main__")
