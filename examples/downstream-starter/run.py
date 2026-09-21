#!/usr/bin/env python3
"""Stage public Cargo inputs, build the guest, and run the standalone host (3.11+)."""
import argparse
import copy
import json
import os
from pathlib import Path
import shutil
import subprocess
import tomllib

HERE = Path(__file__).resolve().parent


def value(item):
    if isinstance(item, dict):
        return "{ " + ", ".join(json.dumps(k) + " = " + value(v) for k, v in item.items()) + " }"
    if isinstance(item, list):
        return "[" + ", ".join(value(v) for v in item) + "]"
    return json.dumps(item)


def manifest(data):
    return "\n".join(json.dumps(k) + " = " + value(v) for k, v in data.items()) + "\n"


def stage(root, env):
    workspace = tomllib.loads((root / "Cargo.toml").read_text())["workspace"]
    def cargo(*args):
        return subprocess.check_output(["cargo", *args, "--offline"], cwd=root, env=env, text=True)
    metadata = json.loads(cargo("metadata", "--no-deps", "--format-version", "1"))
    packages = [p for p in metadata["packages"] if Path(p["manifest_path"]).parent.parent == root / "crates"]
    destinations = {p["name"]: HERE / "vendor" / p["name"] for p in packages}
    if not packages:
        raise RuntimeError("No public crates found in the supplied Cell checkout")
    for package in packages:
        source = Path(package["manifest_path"]).parent
        destination = destinations[package["name"]]
        files = cargo("package", "-p", package["name"], "--list", "--allow-dirty").splitlines()
        if destination.exists():
            shutil.rmtree(destination)
        destination.mkdir(parents=True)
        for name in files:
            if name in {"Cargo.toml", "Cargo.toml.orig", "Cargo.lock", ".cargo_vcs_info.json"}:
                continue
            original = source / name
            if not original.is_file():
                raise RuntimeError(f"Missing Cargo package input: {original}")
            target = destination / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(original, target)
        data = tomllib.loads((source / "Cargo.toml").read_text())
        for key, item in list(data["package"].items()):
            if isinstance(item, dict) and item.get("workspace"):
                data["package"][key] = copy.deepcopy(workspace["package"][key])
        data.pop("dev-dependencies", None)
        def resolve(table):
            table.pop("dev-dependencies", None)
            for section in ("dependencies", "build-dependencies"):
                for key, item in list(table.get(section, {}).items()):
                    if isinstance(item, dict) and item.get("workspace"):
                        local = {k: v for k, v in item.items() if k != "workspace"}
                        inherited = copy.deepcopy(workspace["dependencies"][key])
                        item = {"version": inherited} if isinstance(inherited, str) else inherited
                        item["features"] = list(dict.fromkeys(item.get("features", []) + local.pop("features", [])))
                        item.update(local)
                        table[section][key] = item
                    if isinstance(item, dict) and "path" in item:
                        name = item.get("package", key)
                        if "version" not in item or name not in destinations:
                            raise RuntimeError(f"Unsupported local dependency: {key}")
                        item["path"] = "../" + name
        resolve(data)
        for table in data.get("target", {}).values():
            resolve(table)
        if "lints" in data and data["lints"].get("workspace"):
            data["lints"] = copy.deepcopy(workspace["lints"])
        (destination / "Cargo.toml").write_text(manifest(data))
    deps = HERE / "guest" / "wit" / "deps"
    if deps.exists():
        shutil.rmtree(deps)
    for name, directory in (("cell-capability-log", "cell-log"), ("cell-capability-fs", "cell-fs")):
        shutil.copytree(destinations[name] / "wit", deps / directory)
    # Seed transitive resolution from the checkout, then retain the starter's own lock.
    if not (HERE / "Cargo.lock").exists():
        shutil.copy2(root / "Cargo.lock", HERE / "Cargo.lock")
    print("Staged Cargo-listed public inputs and capability WIT.", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("cell_checkout", type=Path)
    parser.add_argument("--offline", action="store_true", help="use only cached registry dependencies")
    parser.add_argument("--stage-only", action="store_true")
    args = parser.parse_args()
    root = args.cell_checkout.resolve(strict=True)
    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = str(HERE / "target")
    stage(root, env)
    if args.stage_only:
        return
    flags = ["--offline"] if args.offline else []
    def run(*command):
        subprocess.run(command, cwd=HERE, env=env, check=True)
    run("cargo", "build", "-p", "downstream-guest", "--target", "wasm32-unknown-unknown", *flags)
    fixture = HERE / "target" / "read-only-root"
    fixture.mkdir(parents=True, exist_ok=True)
    (fixture / "input.txt").write_text("hello from the downstream fixture\n")
    run("cargo", "run", "-p", "downstream-host", *flags, "--",
        str(HERE / "target/wasm32-unknown-unknown/debug/downstream_guest.wasm"), str(fixture))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        raise SystemExit(f"Starter failed: {error}") from error
