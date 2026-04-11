---
layout: default
title: Usage Examples
description: Practical usage examples and best practices for Sunbeam CLI
parent: index
toc: true
---

# Usage Examples and Best Practices

This section provides practical usage examples and best practices for working with Sunbeam CLI.

## Common Workflows

### Starting a New Development Session

```bash
# Start the local cluster
sunbeam up

# Check cluster status
sunbeam status

# Apply manifests with local domain
sunbeam apply

# Seed secrets
sunbeam seed

# Verify everything is working
sunbeam verify
```

### Production Deployment

```bash
# Set environment variables
export SUNBEAM_SSH_HOST=user@your-server.example.com

# Apply production manifests with custom domain
sunbeam apply --env production --domain sunbeam.pt --email ops@sunbeam.pt

# Check production status
sunbeam status --env production

# Restart specific service in production
sunbeam restart ory/kratos --env production
```

### Debugging a Service Issue

```bash
# Check service status
sunbeam status ory/kratos

# View recent logs
sunbeam logs ory/kratos

# Stream logs in real-time
sunbeam logs ory/kratos -f

# Get pod details
sunbeam get ory/kratos-abc123

# Restart the service
sunbeam restart ory/kratos

# Run health checks
sunbeam check ory/kratos
```

## Advanced Usage Patterns

### Partial Manifest Application

```bash
# Apply only specific namespace
sunbeam apply devtools

# Apply multiple namespaces individually
sunbeam apply ingress
sunbeam apply ory

# Apply all manifests
sunbeam apply
```

### Build and Deploy Workflow

```bash
# Build an image
sunbeam build proxy

# Build and push to registry
sunbeam build proxy --push

# Build, push, and deploy
sunbeam build proxy --deploy

# Build multiple components
sunbeam build people-backend --push
sunbeam build people-frontend --push
sunbeam build proxy --deploy
```

### User Management Workflow

```bash
# Create a new user
sunbeam user create user@example.com --name "User Name"

# Set user password
sunbeam user set-password user@example.com "securepassword123"

# List all users
sunbeam user list

# Get user details
sunbeam user get user@example.com

# Disable a user (lockout)
sunbeam user disable user@example.com

# Re-enable a user
sunbeam user enable user@example.com

# Generate recovery link
sunbeam user recover user@example.com
```

## Best Practices

### Environment Management

**Use separate contexts for different environments:**

```bash
# Local development (default)
sunbeam status

# Production with explicit context
sunbeam status --env production --context production
```

**Set domain appropriately:**

```bash
# Local development uses automatic Lima IP domain
sunbeam apply

# Production requires explicit domain
sunbeam apply --env production --domain sunbeam.pt
```

### Service Management

**Use targeted restarts:**

```bash
# Restart specific service instead of all
sunbeam restart ory/kratos

# Restart namespace instead of entire cluster
sunbeam restart ory
```

**Monitor service health:**

```bash
# Check status before and after operations
sunbeam status ory

# Use health checks for functional verification
sunbeam check ory/kratos
```

### Manifest Management

**Apply in logical order:**

```bash
# Apply infrastructure first
sunbeam apply ingress
sunbeam apply data

# Then apply application layers
sunbeam apply ory
sunbeam apply devtools
```

**Handle webhook dependencies:**

```bash
# cert-manager must be ready before applying certificates
# Sunbeam automatically handles this with convergence passes
sunbeam apply
```

### Debugging Techniques

**Use kubectl passthrough for advanced debugging:**

```bash
# Direct kubectl access
sunbeam k8s get pods -A -o wide

# Describe resources
sunbeam k8s describe pod ory/kratos-abc123

# View events
sunbeam k8s get events --sort-by=.metadata.creationTimestamp
```

**Check VSO secret sync status:**

```bash
# Status command includes VSO sync information
sunbeam status

# Look for sync indicators in output
# ✓ indicates synced, ✗ indicates issues
```

### Configuration Management

**Use environment variables for sensitive data:**

```bash
# Set ACME email via environment
export SUNBEAM_ACME_EMAIL=ops@sunbeam.pt
sunbeam apply --env production --domain sunbeam.pt
```

**Manage multiple clusters:**

```bash
# Use context overrides for different clusters
sunbeam status --context cluster1
sunbeam status --context cluster2
```

## Troubleshooting Guide

### Common Issues and Solutions

**Issue: Cluster not starting**

```bash
# Check Lima VM status
limactl list
limactl start sunbeam

# Check cluster components
sunbeam k8s get nodes
sunbeam k8s get pods -A
```

**Issue: Manifest apply failures**

```bash
# Check for webhook issues
sunbeam k8s get validatingwebhookconfigurations
sunbeam k8s get mutatingwebhookconfigurations

# Check cert-manager status
sunbeam status cert-manager
sunbeam logs cert-manager/cert-manager
```

**Issue: Secret sync problems**

```bash
# Check VSO status
sunbeam status

# Check OpenBao connection
sunbeam bao status

# Verify secrets exist
sunbeam k8s get vaultstaticsecrets -A
sunbeam k8s get vaultdynamicsecrets -A
```

**Issue: Service not responding**

```bash
# Check service status
sunbeam status ory/kratos

# Check logs
sunbeam logs ory/kratos -f

# Check service and endpoint
sunbeam k8s get service ory/kratos
sunbeam k8s get endpoints ory/kratos

# Restart service
sunbeam restart ory/kratos
```

## Integration Patterns

### CI/CD Integration

```yaml
# Example GitHub Actions workflow
- name: Deploy to production
  env:
    SUNBEAM_SSH_HOST: ${{ secrets.PRODUCTION_SSH_HOST }}
  run: |
    sunbeam apply --env production --domain sunbeam.pt --email ops@sunbeam.pt
    sunbeam check
```

### Local Development Workflow

```bash
# Start fresh each day
sunbeam down
sunbeam up
sunbeam apply
sunbeam seed

# During development
sunbeam restart ory/kratos
sunbeam logs ory/kratos -f

# End of day cleanup
sunbeam down
```

### Team Collaboration

```bash
# Bootstrap new environment
sunbeam up
sunbeam apply
sunbeam seed
sunbeam bootstrap

# Onboard new team member
sunbeam user create newuser@company.com --name "New User"
sunbeam user set-password newuser@company.com "temporary123"

# Share access information
sunbeam user recover newuser@company.com
```

## Performance Tips

**Targeted operations:**

```bash
# Apply only what's needed
sunbeam apply ory  # Instead of sunbeam apply

# Restart only affected services
sunbeam restart ory/kratos  # Instead of sunbeam restart
```

**Use namespace filtering:**

```bash
# Focus on specific areas
sunbeam status ory
sunbeam logs ory/kratos
```

**Monitor resource usage:**

```bash
# Check cluster resource usage
sunbeam k8s top nodes
sunbeam k8s top pods -A

# Check specific namespace
sunbeam k8s top pods -n ory
```

## Security Best Practices

**Secret management:**

```bash
# Use sunbeam's built-in secret management
sunbeam seed
sunbeam verify

# Avoid hardcoding secrets in manifests
```

**User management:**

```bash
# Regularly audit users
sunbeam user list

# Disable unused accounts
sunbeam user disable olduser@company.com

# Rotate passwords regularly
sunbeam user set-password user@company.com "newsecurepassword"
```

**Production access:**

```bash
# Use SSH host environment variable securely
export SUNBEAM_SSH_HOST=user@server.example.com

# Never commit SSH host in code
# Use secret management for production credentials
```

## Monitoring and Maintenance

**Regular health checks:**

```bash
# Daily health monitoring
sunbeam status
sunbeam check

# Weekly deep checks
sunbeam verify
sunbeam k8s get events -A --sort-by=.metadata.creationTimestamp
```

**Log management:**

```bash
# Check logs for errors
sunbeam logs ory/kratos | grep ERROR
sunbeam logs ory/hydra | grep Exception

# Monitor specific services
sunbeam logs ingress/nginx-ingress -f
```

**Resource cleanup:**

```bash
# Clean up completed jobs
sunbeam k8s delete jobs --field-selector=status.successful=1

# Remove old resources
sunbeam k8s delete pods --field-selector=status.phase=Succeeded
```

## Advanced Patterns

### Custom Domain Management

```bash
# Discover current domain
CURRENT_DOMAIN=$(sunbeam k8s get cm gitea-inline-config -n devtools -o jsonpath='{.data.server}' | grep DOMAIN= | cut -d= -f2)

# Apply with custom domain
sunbeam apply --domain custom.example.com
```

### Multi-environment Testing

```bash
# Test changes in local first
sunbeam apply --env local
sunbeam check

# Then apply to production
sunbeam apply --env production --domain sunbeam.pt
sunbeam check --env production
```

### Integration with Other Tools

```bash
# Use with kubectl
kubectl --context=sunbeam get pods -A

# Use with helm
helm --kube-context=sunbeam list -A

# Use with k9s (set context first)
sunbeam k8s config use-context sunbeam
k9s
```