#!/usr/bin/env python3
"""Run an isolated, repeatable downstream smoke test (Python 3.11+)."""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import tomllib

HERE = Path(__file__).resolve().parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("cell_checkout", type=Path)
    parser.add_argument("--offline", action="store_true")
    args = parser.parse_args()
    root = args.cell_checkout.resolve(strict=True)
    work = HERE / "smoke-work"
    work.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(dir=work, prefix="standalone-") as directory:
        starter = Path(directory)
        for name in ("Cargo.toml", "run.py", "host", "guest"):
            source = HERE / name
            if source.is_dir():
                shutil.copytree(source, starter / name, ignore=shutil.ignore_patterns("deps"))
            else:
                shutil.copy2(source, starter / name)
        env = os.environ.copy()

        command = [sys.executable, str(starter / "run.py"), str(root)]
        if args.offline:
            command.append("--offline")
        result = subprocess.run(command, cwd=starter, env=env, text=True,
                                stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        print(result.stdout, end="")
        if result.returncode:
            raise RuntimeError(f"Workflow exited with {result.returncode}")
        assert "downstream guest started" in result.stdout
        assert "read=hello from the downstream fixture; write=permission-denied" in result.stdout
        fixture = starter / "target/read-only-root"
        assert not (fixture / "denied.txt").exists()
        for name, dep in (("cell-capability-log", "cell-log"), ("cell-capability-fs", "cell-fs")):
            source = starter / "vendor" / name / "wit"
            for wit in source.rglob("*.wit"):
                assert wit.read_bytes() == (starter / "guest/wit/deps" / dep / wit.relative_to(source)).read_bytes()
        metadata = subprocess.check_output(
            ["cargo", "metadata", "--offline", "--format-version", "1", "--no-deps"], cwd=starter, text=True)
        import json
        assert Path(json.loads(metadata)["workspace_root"]) == starter
        for path in (starter / "vendor").glob("*/Cargo.toml"):
            data = tomllib.loads(path.read_text())
            for section in ("dependencies", "build-dependencies"):
                for dep in data.get(section, {}).values():
                    if isinstance(dep, dict) and "path" in dep:
                        assert (path.parent / dep["path"]).resolve().is_relative_to(starter / "vendor")
        host = starter / "target/debug/downstream-host"
        wasm = starter / "target/wasm32-unknown-unknown/debug/downstream_guest.wasm"
        for arguments, expected in (([], "usage:"), ([str(wasm), str(fixture)], "guest: read:")):
            if arguments:
                (fixture / "input.txt").unlink()
            failed = subprocess.run([str(host), *arguments], capture_output=True, text=True)
            assert failed.returncode != 0 and expected in failed.stderr, failed.stderr
        print("Smoke passed: standalone workspace, staged WIT, typed result, denied write, and host errors.")


if __name__ == "__main__":
    main()
