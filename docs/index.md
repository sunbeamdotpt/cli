---
layout: default
title: Sunbeam CLI Documentation
description: Comprehensive documentation for the Sunbeam local dev stack manager
toc: true
---

# Sunbeam CLI Documentation

Welcome to the comprehensive documentation for **Sunbeam CLI** - a powerful local development stack manager for Kubernetes-based applications.

## Overview

Sunbeam is a command-line tool designed to simplify the management of local development environments for Kubernetes applications. It provides a comprehensive suite of commands to handle cluster operations, service management, secret handling, and more.

## Key Features

- **Cluster Management**: Bring up and tear down local Kubernetes clusters
- **Service Operations**: Manage services, logs, and health checks
- **Secret Management**: Secure credential handling with OpenBao integration
- **Manifest Management**: Kustomize-based manifest application with domain substitution
- **User Management**: Identity management for development environments
- **Tool Bundling**: Automatic download and management of required binaries

## Getting Started

### Installation

```bash
# Clone the repository
git clone https://src.sunbeam.pt/studio/cli.git
cd cli

# Install dependencies
pip install .

# Verify installation
sunbeam --help
```

### Basic Usage

```bash
# Start your local cluster
sunbeam up

# Check cluster status
sunbeam status

# Apply manifests
sunbeam apply

# View service logs
sunbeam logs ory/kratos
```

## Documentation Structure

- **[CLI Reference](cli-reference)**: Complete command reference
- **[Core Modules](core-modules)**: Detailed module documentation
- **[Architecture](architecture)**: System architecture and design
- **[Usage Examples](usage-examples)**: Practical usage scenarios
- **[Development Guide](development)**: Contributing and extending Sunbeam

## Support

For issues, questions, or contributions, please refer to the [Git repository](https://src.sunbeam.pt/studio/cli).