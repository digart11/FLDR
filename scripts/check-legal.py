"""Verify FLDR release licensing evidence."""
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

if __name__ == "__main__":
    raise SystemExit(subprocess.call([
        sys.executable,
        str(ROOT / "scripts/generate-third-party.py"),
        "--verify-release",
    ], cwd=ROOT))
