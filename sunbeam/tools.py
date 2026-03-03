"""Binary bundler — downloads kubectl, kustomize, helm at pinned versions.

Binaries are cached in ~/.local/share/sunbeam/bin/ and SHA256-verified.
"""
import hashlib
import io
import os
import stat
import subprocess
import tarfile
import urllib.request
from pathlib import Path

CACHE_DIR = Path.home() / ".local/share/sunbeam/bin"

TOOLS: dict[str, dict] = {
    "kubectl": {
        "version": "v1.32.2",
        "url": "https://dl.k8s.io/release/v1.32.2/bin/darwin/arm64/kubectl",
        "sha256": "",  # set to actual hash; empty = skip verify
    },
    "kustomize": {
        "version": "v5.8.1",
        "url": "https://github.com/kubernetes-sigs/kustomize/releases/download/kustomize%2Fv5.8.1/kustomize_v5.8.1_darwin_arm64.tar.gz",
        "sha256": "",
        "extract": "kustomize",
    },
    "helm": {
        "version": "v4.1.0",
        "url": "https://get.helm.sh/helm-v4.1.0-darwin-arm64.tar.gz",
        "sha256": "82f7065bf4e08d4c8d7881b85c0a080581ef4968a4ae6df4e7b432f8f7a88d0c",
        "extract": "darwin-arm64/helm",
    },
}


def _sha256(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


def ensure_tool(name: str) -> Path:
    """Return path to cached binary, downloading + verifying if needed.

    Re-downloads automatically when the pinned version in TOOLS changes.
    A <name>.version sidecar file records the version of the cached binary.
    """
    if name not in TOOLS:
        raise ValueError(f"Unknown tool: {name}")
    spec = TOOLS[name]
    CACHE_DIR.mkdir(parents=True, exist_ok=True)
    dest = CACHE_DIR / name
    version_file = CACHE_DIR / f"{name}.version"

    expected_sha = spec.get("sha256", "")
    expected_version = spec.get("version", "")

    # Use cached binary if version matches (or no version pinned) and SHA passes
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

    # Download
    url = spec["url"]
    with urllib.request.urlopen(url) as resp:  # noqa: S310
        data = resp.read()

    # Extract from tar.gz if needed
    extract_path = spec.get("extract")
    if extract_path:
        with tarfile.open(fileobj=io.BytesIO(data)) as tf:
            member = tf.getmember(extract_path)
            fobj = tf.extractfile(member)
            binary_data = fobj.read()
    else:
        binary_data = data

    # Write to cache
    dest.write_bytes(binary_data)

    # Verify SHA256 (after extraction)
    if expected_sha:
        actual = _sha256(dest)
        if actual != expected_sha:
            dest.unlink()
            raise RuntimeError(
                f"SHA256 mismatch for {name}: expected {expected_sha}, got {actual}"
            )

    # Make executable
    dest.chmod(dest.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    # Record version so future calls skip re-download when version unchanged
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
    # kustomize needs helm on PATH for helm chart rendering
    if name == "kustomize":
        if "helm" in TOOLS:
            ensure_tool("helm")  # ensure bundled helm is present before kustomize runs
        env["PATH"] = str(CACHE_DIR) + os.pathsep + env.get("PATH", "")
    return subprocess.run([str(bin_path), *args], env=env, **kwargs)
