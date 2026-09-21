#!/usr/bin/env python3
"""Check Cargo's distributable file boundary outside the repository (Python 3.11+)."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[2]
FIXTURE = Path(__file__).resolve().parent


def cargo(*args):
    return subprocess.check_output(["cargo", *args], cwd=ROOT, text=True)


def value(item):
    if isinstance(item, dict):
        return "{ " + ", ".join(json.dumps(k) + " = " + value(v) for k, v in item.items()) + " }"
    if isinstance(item, list):
        return "[" + ", ".join(value(v) for v in item) + "]"
    return json.dumps(item)


def manifest(data):
    # Inline tables preserve dependency features/options without a TOML writer dependency.
    return "\n".join(json.dumps(k) + " = " + value(v) for k, v in data.items()) + "\n"


def main():
    workspace = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]
    metadata = json.loads(cargo("metadata", "--no-deps", "--format-version", "1", "--offline"))
    packages = [p for p in metadata["packages"] if Path(p["manifest_path"]).parent.parent == ROOT / "crates"]
    with tempfile.TemporaryDirectory(prefix="cell-external-") as directory:
        stage = Path(directory)
        destinations = {p["name"]: stage / p["name"] for p in packages}
        for package in packages:
            source = Path(package["manifest_path"]).parent
            destination = destinations[package["name"]]
            files = cargo("package", "-p", package["name"], "--list", "--allow-dirty", "--offline").splitlines()
            for name in files:
                # Cargo generates these; create the standalone manifest below.
                if name in {"Cargo.toml", "Cargo.toml.orig", "Cargo.lock", ".cargo_vcs_info.json"}:
                    continue
                original = source / name
                if not original.is_file():
                    raise RuntimeError(f"Missing package input: {original}")
                target = destination / name
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(original, target)
            data = tomllib.loads((source / "Cargo.toml").read_text())
            for key, item in list(data["package"].items()):
                if isinstance(item, dict) and item.get("workspace"):
                    data["package"][key] = workspace["package"][key]
            # Dev fixtures are repository tests, not library build inputs.
            data.pop("dev-dependencies", None)
            for section in ("dependencies", "build-dependencies"):
                for key, item in list(data.get(section, {}).items()):
                    if isinstance(item, dict) and item.get("workspace"):
                        item = workspace["dependencies"][key]
                        data[section][key] = item
                    if isinstance(item, dict) and "path" in item:
                        name = item.get("package", key)
                        assert "version" in item, f"Path-only dependency: {key}"
                        item["path"] = str(destinations[name])
            (destination / "Cargo.toml").write_text(manifest(data))
        consumer = stage / "consumer"
        shutil.copytree(FIXTURE / "src", consumer / "src")
        data = tomllib.loads((FIXTURE / "Cargo.toml").read_text())
        # Simulate unpublished registry packages, using only Cargo-listed files.
        data["patch"] = {"crates-io": {name: {"path": str(path)} for name, path in destinations.items()}}
        (consumer / "Cargo.toml").write_text(manifest(data))
        # Preserve the repository's third-party dependency resolution where possible.
        shutil.copy2(ROOT / "Cargo.lock", consumer / "Cargo.lock")
        environment = os.environ.copy()
        environment["CARGO_TARGET_DIR"] = str(ROOT / "target" / "external-consumer")
        subprocess.run(["cargo", "check", "--offline", "--manifest-path", str(consumer / "Cargo.toml")],
                       cwd=stage, env=environment, check=True)
        print("External consumer passed using isolated Cargo package contents.")


if __name__ == "__main__":
    main()
