---
layout: default
title: API Reference
description: Comprehensive API documentation for all Sunbeam modules
parent: index
toc: true
---

# API Reference

This section provides comprehensive API documentation for all Sunbeam modules, including function signatures, parameters, return values, and usage examples.

## CLI Module (`sunbeam.cli`)

### Main Function

```python
def main() -> None
```

**Description:** Main entry point for the Sunbeam CLI. Parses arguments and dispatches to appropriate command handlers.

**Environment Variables:**
- `SUNBEAM_SSH_HOST`: Required for production environment

**Global Arguments:**
- `--env`: Target environment (`local` or `production`)
- `--context`: kubectl context override
- `--domain`: Domain suffix for production
- `--email`: ACME email for cert-manager

### Command Dispatch

The main function uses a dispatch pattern to route commands to their handlers:

```python
# Example dispatch pattern
if args.verb == "up":
    from sunbeam.cluster import cmd_up
    cmd_up()
elif args.verb == "status":
    from sunbeam.services import cmd_status
    cmd_status(args.target)
# ... additional commands
```

## Kubernetes Module (`sunbeam.kube`)

### Context Management

```python
def set_context(ctx: str, ssh_host: str = "") -> None
```

**Parameters:**
- `ctx`: Kubernetes context name
- `ssh_host`: SSH host for production tunnel (optional)

**Description:** Sets the active kubectl context and SSH host for production environments.

```python
def context_arg() -> str
```

**Returns:** `--context=<active>` string for subprocess calls

### Kubernetes Operations

```python
def kube(*args, input=None, check=True) -> subprocess.CompletedProcess
```

**Parameters:**
- `*args`: kubectl arguments
- `input`: stdin input (optional)
- `check`: Whether to raise exception on non-zero exit (default: True)

**Returns:** CompletedProcess result

**Description:** Run kubectl against active context with automatic SSH tunnel management.

```python
def kube_out(*args) -> str
```

**Parameters:**
- `*args`: kubectl arguments

**Returns:** stdout output (empty string on failure)

```python
def kube_ok(*args) -> bool
```

**Parameters:**
- `*args`: kubectl arguments

**Returns:** True if command succeeds

```python
def kube_apply(manifest: str, *, server_side: bool = True) -> None
```

**Parameters:**
- `manifest`: YAML manifest string
- `server_side`: Use server-side apply (default: True)

**Description:** Apply YAML manifest via kubectl.

### Resource Management

```python
def ns_exists(ns: str) -> bool
```

**Parameters:**
- `ns`: Namespace name

**Returns:** True if namespace exists

```python
def ensure_ns(ns: str) -> None
```

**Parameters:**
- `ns`: Namespace name

**Description:** Ensure namespace exists, creating if necessary.

```python
def create_secret(ns: str, name: str, **literals) -> None
```

**Parameters:**
- `ns`: Namespace name
- `name`: Secret name
- `**literals`: Key-value pairs for secret data

**Description:** Create or update Kubernetes secret idempotently.

### Kustomize Integration

```python
def kustomize_build(overlay: Path, domain: str, email: str = "") -> str
```

**Parameters:**
- `overlay`: Path to kustomize overlay directory
- `domain`: Domain suffix for substitution
- `email`: ACME email for substitution (optional)

**Returns:** Built manifest YAML with substitutions applied

**Description:** Build kustomize overlay with domain/email substitution and helm support.

### SSH Tunnel Management

```python
def ensure_tunnel() -> None
```

**Description:** Open SSH tunnel to production cluster if needed. Automatically checks if tunnel is already open.

### Utility Functions

```python
def parse_target(s: str | None) -> tuple[str | None, str | None]
```

**Parameters:**
- `s`: Target string in format `namespace/name` or `namespace`

**Returns:** Tuple of (namespace, name)

**Examples:**
```python
parse_target("ory/kratos")    # ("ory", "kratos")
parse_target("devtools")      # ("devtools", None)
parse_target(None)             # (None, None)
```

```python
def domain_replace(text: str, domain: str) -> str
```

**Parameters:**
- `text`: Input text containing DOMAIN_SUFFIX placeholders
- `domain`: Domain to substitute

**Returns:** Text with DOMAIN_SUFFIX replaced by domain

```python
def get_lima_ip() -> str
```

**Returns:** IP address of Lima VM

**Description:** Discover Lima VM IP using multiple fallback methods.

```python
def get_domain() -> str
```

**Returns:** Active domain from cluster state

**Description:** Discover domain from various cluster sources with fallback to Lima IP.

### Command Functions

```python
def cmd_k8s(kubectl_args: list[str]) -> int
```

**Parameters:**
- `kubectl_args`: Arguments to pass to kubectl

**Returns:** Exit code

**Description:** Transparent kubectl passthrough for active context.

```python
def cmd_bao(bao_args: list[str]) -> int
```

**Parameters:**
- `bao_args`: Arguments to pass to bao CLI

**Returns:** Exit code

**Description:** Run bao CLI inside OpenBao pod with root token injection.

## Manifests Module (`sunbeam.manifests`)

### Main Command

```python
def cmd_apply(env: str = "local", domain: str = "", email: str = "", namespace: str = "") -> None
```

**Parameters:**
- `env`: Environment (`local` or `production`)
- `domain`: Domain suffix (required for production)
- `email`: ACME email for cert-manager
- `namespace`: Limit apply to specific namespace (optional)

**Description:** Build kustomize overlay, apply domain/email substitution, and apply manifests with convergence handling.

### Cleanup Functions

```python
def pre_apply_cleanup(namespaces=None) -> None
```

**Parameters:**
- `namespaces`: List of namespaces to clean (default: all managed namespaces)

**Description:** Clean up immutable resources and stale secrets before apply.

### ConfigMap Management

```python
def _snapshot_configmaps() -> dict
```

**Returns:** Dictionary of `{ns/name: resourceVersion}` for all ConfigMaps

```python
def _restart_for_changed_configmaps(before: dict, after: dict) -> None
```

**Parameters:**
- `before`: ConfigMap snapshot before apply
- `after`: ConfigMap snapshot after apply

**Description:** Restart deployments that mount changed ConfigMaps.

### Webhook Management

```python
def _wait_for_webhook(ns: str, svc: str, timeout: int = 120) -> bool
```

**Parameters:**
- `ns`: Namespace of webhook service
- `svc`: Service name
- `timeout`: Maximum wait time in seconds

**Returns:** True if webhook becomes ready

**Description:** Wait for webhook service endpoint to become available.

### Utility Functions

```python
def _filter_by_namespace(manifests: str, namespace: str) -> str
```

**Parameters:**
- `manifests`: Multi-document YAML string
- `namespace`: Namespace to filter by

**Returns:** Filtered YAML containing only resources for specified namespace

**Description:** Filter manifests by namespace using string matching.

## Services Module (`sunbeam.services`)

### Status Command

```python
def cmd_status(target: str | None) -> None
```

**Parameters:**
- `target`: Optional target in format `namespace` or `namespace/name`

**Description:** Show pod health across managed namespaces with VSO sync status.

### Log Command

```python
def cmd_logs(target: str, follow: bool) -> None
```

**Parameters:**
- `target`: Target in format `namespace/name`
- `follow`: Whether to stream logs

**Description:** Stream logs for a service using label-based pod selection.

### Get Command

```python
def cmd_get(target: str, output: str = "yaml") -> None
```

**Parameters:**
- `target`: Target in format `namespace/name`
- `output`: Output format (`yaml`, `json`, or `wide`)

**Description:** Get raw kubectl output for a resource.

### Restart Command

```python
def cmd_restart(target: str | None) -> None
```

**Parameters:**
- `target`: Optional target in format `namespace` or `namespace/name`

**Description:** Rolling restart of services. If no target, restarts all managed services.

### VSO Sync Status

```python
def _vso_sync_status() -> None
```

**Description:** Print VaultStaticSecret and VaultDynamicSecret sync health status.

## Tools Module (`sunbeam.tools`)

### Tool Management

```python
def ensure_tool(name: str) -> Path
```

**Parameters:**
- `name`: Tool name (must be in TOOLS dictionary)

**Returns:** Path to executable

**Raises:**
- `ValueError`: Unknown tool
- `RuntimeError`: SHA256 verification failure

**Description:** Ensure tool is available, downloading and verifying if necessary.

```python
def run_tool(name: str, *args, **kwargs) -> subprocess.CompletedProcess
```

**Parameters:**
- `name`: Tool name
- `*args`: Arguments to pass to tool
- `**kwargs`: Additional subprocess.run parameters

**Returns:** CompletedProcess result

**Description:** Run tool, ensuring it's downloaded first. Handles PATH setup for kustomize/helm integration.

### Internal Utilities

```python
def _sha256(path: Path) -> str
```

**Parameters:**
- `path`: Path to file

**Returns:** SHA256 hash of file contents

**Description:** Calculate SHA256 hash (internal utility).

## Output Module (`sunbeam.output`)

### Output Functions

```python
def step(msg: str) -> None
```

**Parameters:**
- `msg`: Message to display

**Description:** Print step header with `==>` prefix.

```python
def ok(msg: str) -> None
```

**Parameters:**
- `msg`: Message to display

**Description:** Print success/info message with indentation.

```python
def warn(msg: str) -> None
```

**Parameters:**
- `msg`: Warning message

**Description:** Print warning to stderr.

```python
def die(msg: str) -> None
```

**Parameters:**
- `msg`: Error message

**Description:** Print error to stderr and exit with code 1.

```python
def table(rows: list[list[str]], headers: list[str]) -> str
```

**Parameters:**
- `rows`: List of row data
- `headers`: List of column headers

**Returns:** Formatted table string

**Description:** Generate aligned text table with automatic column sizing.

## Usage Patterns

### Typical Command Flow

```python
# CLI entry point
from sunbeam.cli import main
main()

# Module usage example
from sunbeam.kube import kube, kube_out, set_context
from sunbeam.output import step, ok, warn

# Set context
set_context("sunbeam")

# Check cluster status
step("Checking cluster status...")
if kube_ok("get", "nodes"):
    ok("Cluster is ready")
    node_count = kube_out("get", "nodes", "--no-headers").count("\n") + 1
    ok(f"Found {node_count} nodes")
else:
    warn("Cluster not ready")
```

### Error Handling Pattern

```python
from sunbeam.output import step, ok, die
from sunbeam.kube import kube_ok

step("Validating cluster requirements...")

# Check required components
required = [
    ("kube-system", "kube-dns"),
    ("ingress-nginx", "ingress-nginx-controller")
]

for ns, deployment in required:
    if not kube_ok("get", "deployment", deployment, "-n", ns):
        die(f"Required deployment {ns}/{deployment} not found")

ok("All requirements satisfied")
```

### Tool Usage Pattern

```python
from sunbeam.tools import ensure_tool, run_tool

# Ensure kubectl is available
kubectl_path = ensure_tool("kubectl")

# Run kubectl command
result = run_tool("kubectl", "get", "pods", "-A", 
                  capture_output=True, text=True)
print(result.stdout)
```