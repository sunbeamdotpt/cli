# Sunbeam CLI

Kubernetes-based local dev stack manager. Python 3.11+, zero runtime dependencies (only stdlib + subprocess calls to bundled binaries).

## Build & Test

```bash
pip install -e .          # install in editable mode
python -m pytest sunbeam/tests/  # run all tests
```

Tests use `unittest` + `unittest.mock`. Test files live in `sunbeam/tests/test_<module>.py`.

## Critical Rules

- **Do NOT add dependencies.** This project has zero Python dependencies beyond setuptools. All external tools (kubectl, kustomize, helm, buildctl) are downloaded at runtime by `tools.py`. Never `pip install` a library to solve a problem — use stdlib.
- **Do NOT refactor code you weren't asked to change.** Don't rename variables, add type hints, rewrite functions "for clarity," or extract helpers. Touch only what the task requires.
- **Do NOT add abstractions.** No base classes, no factories, no wrapper utilities for one-time operations. Three similar lines is fine.
- **Do NOT over-engineer error handling.** Only validate at system boundaries (user input, subprocess results). Trust internal code.
- **Do NOT create new files** unless absolutely necessary. Prefer editing existing files.
- **Do NOT add comments or docstrings** to code you didn't write or change.
- **Never commit secrets** (.env, credentials, keys).

## Architecture

```
sunbeam/
  __main__.py    → entry point, delegates to cli.py
  cli.py         → argparse verb dispatch (lazy-imports command modules)
  config.py      → ~/.sunbeam.json load/save (SunbeamConfig class)
  output.py      → step/ok/warn/die logging + table formatter
  tools.py       → downloads and caches kubectl/kustomize/helm/buildctl binaries
  kube.py        → kubectl/kustomize wrappers, SSH tunnel, domain substitution
  cluster.py     → Lima VM lifecycle (up/down), kubeconfig, core service install
  manifests.py   → kustomize build + kubectl server-side apply pipeline
  services.py    → service status/logs/restart, managed namespace list
  secrets.py     → OpenBao init/unseal/seed, VSO secret sync
  images.py      → container image build + registry mirror
  checks.py      → functional health checks (CheckResult dataclass)
  users.py       → Kratos identity management (create/delete/lockout)
  gitea.py       → Gitea bootstrap + registry config
  tests/         → unittest tests per module
```

Modules are imported lazily in `cli.py` — each verb imports its command module only when invoked. Do not add top-level imports in `cli.py`.

## Code Style — Follow Existing Patterns Exactly

**Module docstrings:** One-line, starts with a capital letter, uses em-dash to separate topic from description:
```python
"""Service management — status, logs, restart."""
```

**Imports:** stdlib first, then `sunbeam.*` imports. Use `from sunbeam.output import step, ok, warn, die` not `import sunbeam.output`.

**Output/logging:** Use `output.py` functions — never bare `print()`:
```python
from sunbeam.output import step, ok, warn, die

step("Applying manifests")   # section header: "\n==> Applying manifests"
ok("Namespace created")      # info line:     "    Namespace created"
warn("Pod not ready")        # stderr:        "    WARN: Pod not ready"
die("Cluster unreachable")   # stderr + exit(1)
```

**Subprocess calls:** Use `subprocess.run()` directly or the wrappers in `kube.py`:
```python
from sunbeam.kube import kube, kube_out, kube_ok

kube("apply", "-f", path)           # run kubectl, check=True
output = kube_out("get", "pods")    # capture stdout
success = kube_ok("get", "ns/foo")  # returns bool, no exception
```

**Command handlers:** Named `cmd_<verb>(args)`, take the argparse Namespace. Defined in their respective module, imported lazily in `cli.py`.

**Type hints:** Minimal. Use them on function signatures when they add clarity. Don't add them retroactively to existing code.

**Constants:** Module-level `UPPER_SNAKE_CASE`. Lists of managed resources are plain Python lists, not configs or enums.

**Global state:** Module-level private variables (`_context`, `_ssh_host`) set once at startup. Don't introduce new global state without good reason.

**Error flow:** `die()` for fatal errors (prints + exits). Don't raise exceptions for user-facing errors.

## What NOT to Do

- Don't introduce dataclasses, NamedTuples, or TypedDicts unless the task specifically needs a new data structure. The only existing dataclass is `CheckResult` in `checks.py`.
- Don't add logging (the `logging` module). Use `output.py` functions.
- Don't add `__all__` exports to modules.
- Don't add `if __name__ == "__main__"` blocks (entry point is `__main__.py`).
- Don't wrap subprocess calls in try/except when `check=True` already handles failure.
- Don't convert working procedural code to classes.
- Don't add CLI arguments that weren't requested. The argparse setup in `cli.py` is intentionally minimal.
- Don't create utility modules, helper files, or shared abstractions.
- Don't add configuration file formats (YAML, TOML) — config is JSON via `config.py`.
