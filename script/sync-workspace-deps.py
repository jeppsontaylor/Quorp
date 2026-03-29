#!/usr/bin/env python3
from pathlib import Path
import re


ROOT = Path(__file__).resolve().parent.parent
CARGO_TOML = ROOT / "Cargo.toml"
CARGO_LOCK = ROOT / "Cargo.lock"


def load_lock_versions() -> dict[str, set[str]]:
    versions: dict[str, set[str]] = {}
    lock_text = CARGO_LOCK.read_text()
    for match in re.finditer(
        r"\[\[package\]\]\n(?:[^\n]*\n)*?name = \"([^\"]+)\"\nversion = \"([^\"]+)\"",
        lock_text,
    ):
        versions.setdefault(match.group(1), set()).add(match.group(2))
    return versions


def package_name_for_manifest(manifest_path: Path) -> str | None:
    manifest = manifest_path.read_text()
    package_block = re.search(r"(?ms)^\[package\]\n(.*?)(?:\n\[|\Z)", manifest)
    if not package_block:
        return None
    name_match = re.search(
        r"^name\s*=\s*\"([^\"]+)\"",
        package_block.group(1),
        re.MULTILINE,
    )
    if not name_match:
        return None
    return name_match.group(1)


def load_workspace_package_paths() -> dict[str, str]:
    crate_paths: dict[str, str] = {}
    for manifest in ROOT.glob("crates/*/Cargo.toml"):
        package_name = package_name_for_manifest(manifest)
        if package_name is None:
            continue
        crate_paths[package_name] = manifest.parent.relative_to(ROOT).as_posix()
    return crate_paths


def collect_workspace_inherited_dependency_names() -> set[str]:
    dependency_names: set[str] = set()
    for manifest_path in ROOT.rglob("Cargo.toml"):
        if ".git" in manifest_path.parts:
            continue
        manifest = manifest_path.read_text()
        for match in re.finditer(
            r"^([A-Za-z0-9_\-]+)\.workspace\s*=\s*true\s*$",
            manifest,
            re.MULTILINE,
        ):
            dependency_names.add(match.group(1))
        for match in re.finditer(
            r"^([A-Za-z0-9_\-]+)\s*=\s*\{[^\n}]*\bworkspace\s*=\s*true[^\n}]*\}",
            manifest,
            re.MULTILINE,
        ):
            dependency_names.add(match.group(1))

    dependency_names.discard("edition")
    dependency_names.discard("publish")
    return dependency_names


def latest_semver(versions: set[str]) -> str:
    def semver_key(version: str) -> tuple[int, int, int]:
        semver_match = re.match(r"(\d+)\.(\d+)\.(\d+)", version)
        if semver_match:
            return tuple(int(part) for part in semver_match.groups())
        return (0, 0, 0)

    return sorted(versions, key=semver_key)[-1]


def build_workspace_dependencies_section() -> str:
    lock_versions = load_lock_versions()
    crate_paths = load_workspace_package_paths()
    dependency_names = collect_workspace_inherited_dependency_names()

    lines = ["[workspace.dependencies]"]
    for dependency_name in sorted(dependency_names):
        if dependency_name in crate_paths:
            lines.append(
                f'{dependency_name} = {{ path = "{crate_paths[dependency_name]}" }}'
            )
            continue

        if dependency_name in lock_versions:
            version = latest_semver(lock_versions[dependency_name])
            lines.append(f'{dependency_name} = "={version}"')
            continue

        lines.append(f'{dependency_name} = "*"')

    return "\n".join(lines) + "\n"


def main() -> None:
    cargo_toml = CARGO_TOML.read_text()
    section = build_workspace_dependencies_section()
    rewritten = re.sub(
        r"(?ms)^\[workspace\.dependencies\]\n.*\Z",
        section,
        cargo_toml,
    )
    CARGO_TOML.write_text(rewritten)


if __name__ == "__main__":
    main()
