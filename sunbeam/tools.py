"""Binary bundler — downloads kubectl, kustomize, helm, buildctl at pinned versions.

Binaries are cached in ~/.local/share/sunbeam/bin/ and SHA256-verified.
Platform (OS + arch) is detected at runtime so the same package works on
darwin/arm64 (development Mac), darwin/amd64, linux/arm64, and linux/amd64.
"""
import hashlib
import io
import os
import platform
import stat
import subprocess
import tarfile
import urllib.request
from pathlib import Path

CACHE_DIR = Path.home() / ".local/share/sunbeam/bin"

# Tool specs — URL and extract templates use {version}, {os}, {arch}.
# {os}   : darwin | linux
# {arch} : arm64  | amd64
_TOOL_SPECS: dict[str, dict] = {
    "kubectl": {
        "version": "v1.32.2",
        "url": "https://dl.k8s.io/release/{version}/bin/{os}/{arch}/kubectl",
        # plain binary, no archive
    },
    "kustomize": {
        "version": "v5.8.1",
        "url": (
            "https://github.com/kubernetes-sigs/kustomize/releases/download/"
            "kustomize%2F{version}/kustomize_{version}_{os}_{arch}.tar.gz"
        ),
        "extract": "kustomize",
    },
    "helm": {
        "version": "v4.1.0",
        "url": "https://get.helm.sh/helm-{version}-{os}-{arch}.tar.gz",
        "extract": "{os}-{arch}/helm",
        "sha256": {
            "darwin_arm64": "82f7065bf4e08d4c8d7881b85c0a080581ef4968a4ae6df4e7b432f8f7a88d0c",
        },
    },
    "buildctl": {
        "version": "v0.28.0",
        # BuildKit releases: buildkit-v0.28.0.linux.amd64.tar.gz
        "url": (
            "https://github.com/moby/buildkit/releases/download/{version}/"
            "buildkit-{version}.{os}-{arch}.tar.gz"
        ),
        "extract": "bin/buildctl",
    },
}

# Expose as TOOLS for callers that do `if "helm" in TOOLS`.
TOOLS = _TOOL_SPECS


def _detect_platform() -> tuple[str, str]:
    """Return (os_name, arch) for the current host."""
    sys_os = platform.system().lower()
    machine = platform.machine().lower()
    os_name = {"darwin": "darwin", "linux": "linux"}.get(sys_os)
    if not os_name:
        raise RuntimeError(f"Unsupported OS: {sys_os}")
    arch = "arm64" if machine in ("arm64", "aarch64") else "amd64"
    return os_name, arch


def _resolve_spec(name: str) -> dict:
    """Return a tool spec with {os} / {arch} / {version} substituted.

    Uses the module-level TOOLS dict so that tests can patch it.
    """
    if name not in TOOLS:
        raise ValueError(f"Unknown tool: {name}")
    os_name, arch = _detect_platform()
    raw = TOOLS[name]
    version = raw.get("version", "")
    fmt = {"version": version, "os": os_name, "arch": arch}
    spec = dict(raw)
    spec["version"] = version
    spec["url"] = raw["url"].format(**fmt)
    if "extract" in raw:
        spec["extract"] = raw["extract"].format(**fmt)
    # sha256 may be a per-platform dict {"darwin_arm64": "..."} or a plain string.
    sha256_val = raw.get("sha256", {})
    if isinstance(sha256_val, dict):
        spec["sha256"] = sha256_val.get(f"{os_name}_{arch}", "")
    return spec


def _sha256(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


def ensure_tool(name: str) -> Path:
    """Return path to cached binary, downloading + verifying if needed.

    Re-downloads automatically when the pinned version in _TOOL_SPECS changes.
    A <name>.version sidecar file records the version of the cached binary.
    """
    spec = _resolve_spec(name)
    CACHE_DIR.mkdir(parents=True, exist_ok=True)
    dest = CACHE_DIR / name
    version_file = CACHE_DIR / f"{name}.version"

    expected_sha = spec.get("sha256", "")
    expected_version = spec.get("version", "")

    if dest.exists():
        version_ok = (
            not expected_version
            or (version_file.exists() and version_file.read_text().strip() == expected_version)
        )
        sha_ok = not expected_sha or _sha256(dest) == expected_sha
        if version_ok and sha_ok:
            return dest

    # Version mismatch or SHA mismatch — re-download
    if dest.exists():
        dest.unlink()
    if version_file.exists():
        version_file.unlink()

    url = spec["url"]
    with urllib.request.urlopen(url) as resp:  # noqa: S310
        data = resp.read()

    extract_path = spec.get("extract")
    if extract_path:
        with tarfile.open(fileobj=io.BytesIO(data)) as tf:
            member = tf.getmember(extract_path)
            fobj = tf.extractfile(member)
            binary_data = fobj.read()
    else:
        binary_data = data

    dest.write_bytes(binary_data)

    if expected_sha:
        actual = _sha256(dest)
        if actual != expected_sha:
            dest.unlink()
            raise RuntimeError(
                f"SHA256 mismatch for {name}: expected {expected_sha}, got {actual}"
            )

    dest.chmod(dest.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    version_file.write_text(expected_version)
    return dest


def run_tool(name: str, *args, **kwargs) -> subprocess.CompletedProcess:
    """Run a bundled tool, ensuring it is downloaded first.

    For kustomize: prepends CACHE_DIR to PATH so helm is found.
    """
    bin_path = ensure_tool(name)
    env = kwargs.pop("env", None)
    if env is None:
        env = os.environ.copy()
    if name == "kustomize":
        if "helm" in TOOLS:
            ensure_tool("helm")
        env["PATH"] = str(CACHE_DIR) + os.pathsep + env.get("PATH", "")
    return subprocess.run([str(bin_path), *args], env=env, **kwargs)
