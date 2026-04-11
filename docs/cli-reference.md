---
layout: default
title: CLI Reference
description: Complete command reference for Sunbeam CLI
parent: index
toc: true
---

# CLI Reference

The Sunbeam CLI provides a comprehensive set of commands for managing local development environments.

## Command Structure

```bash
sunbeam [global-options] <verb> [verb-options]
```

### Global Options

| Option | Description | Default |
|--------|-------------|---------|
| `--env` | Target environment (`local` or `production`) | `local` |
| `--context` | kubectl context override | Auto-determined |
| `--domain` | Domain suffix for production deploys | `` |
| `--email` | ACME email for cert-manager | `` |

## Commands

### Cluster Management

#### `sunbeam up`
Bring up the full local cluster including Lima VM and Kubernetes.

```bash
sunbeam up
```

#### `sunbeam down`
Tear down the Lima VM and local cluster.

```bash
sunbeam down
```

### Status and Information

#### `sunbeam status [target]`
Show pod health across all namespaces, optionally filtered.

```bash
# All pods
sunbeam status

# Specific namespace
sunbeam status ory

# Specific service
sunbeam status ory/kratos
```

### Manifest Management

#### `sunbeam apply [namespace]`
Build kustomize overlay, apply domain substitution, and apply manifests.

```bash
# Apply all manifests
sunbeam apply

# Apply specific namespace only
sunbeam apply ory

# Production environment with custom domain
sunbeam apply --env production --domain sunbeam.pt
```

### Service Operations

#### `sunbeam logs <ns/name> [-f]`
View logs for a service.

```bash
# View recent logs
sunbeam logs ory/kratos

# Stream logs
sunbeam logs ory/kratos -f
```

#### `sunbeam get <ns/name> [-o yaml|json|wide]`
Get raw kubectl output for a resource.

```bash
# Get pod details
sunbeam get ory/kratos-abc -o yaml
```

#### `sunbeam restart [target]`
Rolling restart of services.

```bash
# Restart all services
sunbeam restart

# Restart specific namespace
sunbeam restart ory

# Restart specific service
sunbeam restart ory/kratos
```

#### `sunbeam check [target]`
Run functional service health checks.

```bash
# Check all services
sunbeam check

# Check specific service
sunbeam check ory/kratos
```

### Build and Deployment

#### `sunbeam build <what> [--push] [--deploy]`
Build artifacts and optionally push and deploy.

```bash
# Build proxy
sunbeam build proxy

# Build and push
sunbeam build proxy --push

# Build, push, and deploy
sunbeam build proxy --deploy
```

Available build targets:
- `proxy`, `integration`, `kratos-admin`, `meet`
- `docs-frontend`, `people-frontend`, `people`

#### `sunbeam mirror`
Mirror amd64-only La Suite images.

```bash
sunbeam mirror
```

### Secret Management

#### `sunbeam seed`
Generate and store all credentials in OpenBao.

```bash
sunbeam seed
```

#### `sunbeam verify`
End-to-end VSO + OpenBao integration test.

```bash
sunbeam verify
```

### Bootstrap Operations

#### `sunbeam bootstrap`
Create Gitea orgs/repos and set up Lima registry.

```bash
sunbeam bootstrap
```

### Kubernetes Operations

#### `sunbeam k8s [kubectl args...]`
Transparent kubectl passthrough.

```bash
# Any kubectl command
sunbeam k8s get pods -A
```

#### `sunbeam bao [bao args...]`
Run bao CLI inside OpenBao pod with root token.

```bash
# Run bao commands
sunbeam bao secrets list
```

### User Management

#### `sunbeam user list [--search email]`
List identities.

```bash
# List all users
sunbeam user list

# Search by email
sunbeam user list --search user@example.com
```

#### `sunbeam user get <target>`
Get identity by email or ID.

```bash
sunbeam user get user@example.com
```

#### `sunbeam user create <email> [--name name] [--schema schema]`
Create identity.

```bash
sunbeam user create user@example.com --name "User Name"
```

#### `sunbeam user delete <target>`
Delete identity.

```bash
sunbeam user delete user@example.com
```

#### `sunbeam user recover <target>`
Generate recovery link.

```bash
sunbeam user recover user@example.com
```

#### `sunbeam user disable <target>`
Disable identity and revoke sessions.

```bash
sunbeam user disable user@example.com
```

#### `sunbeam user enable <target>`
Re-enable a disabled identity.

```bash
sunbeam user enable user@example.com
```

#### `sunbeam user set-password <target> <password>`
Set password for an identity.

```bash
sunbeam user set-password user@example.com newpassword
```