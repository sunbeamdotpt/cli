use crate::error::{Result, SunbeamError};
use crate::{debug, info};
use clap::{Parser, Subcommand};
use clap_complete::Shell;

/// Sunbeam local dev stack manager.
#[derive(Parser, Debug)]
#[command(name = "sunbeam", about = "Sunbeam local dev stack manager")]
pub struct Cli {
    /// Named context to use (overrides current-context from config).
    #[arg(long)]
    pub context: Option<String>,

    /// Domain suffix override (e.g. sunbeam.pt). `None` = use the config
    /// value; `Some(..)` = override, even if the value is empty.
    #[arg(long)]
    pub domain: Option<String>,

    /// ACME email for cert-manager (e.g. ops@sunbeam.pt). Same None-vs-Some
    /// override semantics as `--domain`.
    #[arg(long)]
    pub email: Option<String>,

    /// Log output mode.
    ///
    /// The `RUST_LOG` environment variable overrides the default level filter
    /// (e.g. `RUST_LOG=sunbeam=debug` or `RUST_LOG=trace`).
    #[arg(long, value_enum, default_value_t = crate::logging::LogMode::Line, global = true)]
    pub log_mode: crate::logging::LogMode,

    /// Increase logging verbosity. Use once for debug, twice for trace.
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    /// Suppress non-error output (sets log level to warn).
    #[arg(short, long, global = true, conflicts_with = "verbose")]
    pub quiet: bool,

    #[command(subcommand)]
    /// Verb.
    pub verb: Option<Verb>,
}

/// Top-level CLI subcommands.
#[derive(Subcommand, Debug)]
pub enum Verb {
    /// Full cluster bring-up.
    #[command(long_about = r#"""Bring up the entire Sunbeam stack from zero.

This is the primary command for provisioning a local or remote Kubernetes cluster
with the complete Sunbeam platform. It runs a versioned WFE workflow (version 3)
that orchestrates dozens of steps in dependency order:

  1. Lima VM provisioning (when --use-lima or --profile lima)
  2. Infrastructure: Cilium, cert-manager, Longhorn, CNPG, BuildKit
  3. OpenBao initialization/unseal and KV seeding
  4. PostgreSQL role/database creation and Vault database engine config
  5. Namespace and secret creation
  6. Platform manifests: ingress, identity (Ory), storage, registry, VPN
  7. Application manifests: matrix, wfe, press
  8. Service rollouts and observability
  9. VPN key minting and URL printing

Most steps are idempotent — running `sunbeam up` multiple times is safe and
will only apply changes.

EXAMPLES:
  # Full bring-up on a Lima VM
  sunbeam up --use-lima

  # Re-run after editing manifests (only changes are applied)
  sunbeam up

  # Skip specific namespaces
  sunbeam up --disable matrix --disable press

  # Override a manifest field
  sunbeam up --set deployment/ory/kratos/spec/replicas=3

  # Visualize the workflow DAG as Graphviz DOT
  sunbeam up --graph > up.dot && dot -Tpng up.dot -o up.png

  # Use a profile defined in infra/profiles/<name>.yaml
  sunbeam up --profile minimal
""#)]
    Up {
        /// Override a manifest field (kind/namespace/name/field/path=value).
        #[arg(long = "set")]
        set: Vec<String>,
        /// Disable a resource or pattern (kind/namespace/name or glob).
        #[arg(long)]
        disable: Vec<String>,
        /// Re-enable a resource or pattern.
        #[arg(long)]
        enable: Vec<String>,
        /// Skip the Cilium CNI check.
        #[arg(long)]
        skip_cilium: bool,
        /// Output a Graphviz DOT graph of the workflow and exit.
        #[arg(long)]
        graph: bool,
        /// Use Lima VM for local k3s (shorthand for --profile lima).
        #[arg(long)]
        use_lima: bool,
        /// Profile to load (from infra/profiles/<name>.yaml).
        #[arg(long)]
        profile: Option<String>,
        /// Run in serial mode: longer delays between namespace applies and
        /// more conservative resource usage for tiny single-node clusters.
        #[arg(long)]
        serial: bool,
    },

    /// Full cluster tear-down.
    #[command(long_about = r#"""Tear down the Sunbeam stack.

Deletes Kubernetes namespaces in reverse dependency order. By default, only
application and platform namespaces are removed. Infrastructure namespaces
(cert-manager, longhorn-system) are preserved unless --infra is passed.

The data namespace (Postgres, OpenBao, OpenSearch, Valkey) is deleted by default.
Use --keep-data to preserve it across teardowns.

A confirmation prompt is shown listing every namespace that will be deleted.
Use --yes to skip the prompt for automation.

EXAMPLES:
  # Interactive tear-down (default namespaces only)
  sunbeam down

  # Non-interactive, include infra namespaces
  sunbeam down --yes --infra

  # Tear down but preserve databases and secrets
  sunbeam down --yes --keep-data

  # Tear down a Lima-based local stack
  sunbeam down --yes --use-lima
""#)]
    Down {
        /// Skip confirmation prompt.
        #[arg(long)]
        yes: bool,
        /// Also delete infrastructure namespaces (cert-manager, longhorn-system).
        #[arg(long)]
        infra: bool,
        /// Preserve data namespace (postgres, opensearch).
        #[arg(long)]
        keep_data: bool,
        /// Use Lima VM for local k3s (shorthand for --profile lima).
        #[arg(long)]
        use_lima: bool,
        /// Profile to load (from infra/profiles/<name>.yaml).
        #[arg(long)]
        profile: Option<String>,
    },

    /// Manage sunbeam configuration.
    #[command(long_about = r#"""Manage Sunbeam contexts and configuration.

Sunbeam uses a context system similar to kubectl. Each context stores:
  - domain         — domain suffix for manifest substitution (e.g. sunbeam.pt)
  - infra-dir      — path to infrastructure manifests (kustomize bases)
  - kube-context   — kubectl context name targeting the cluster
  - acme-email     — Let's Encrypt / cert-manager contact email
  - profile        — optional profile reference for workflow tuning
  - vpn-*          — VPN connection settings

Configuration is stored in ~/.sunbeam/config.json.

Use `sunbeam config get` to inspect the active context and all contexts.
Use `sunbeam config use-context <name>` to switch between clusters.

EXAMPLES:
  # Set the active context
  sunbeam config set --domain sunbeam.pt --infra-dir ~/code/infra --kube-context lima-sunbeam

  # Switch contexts
  sunbeam config use-context production

  # View all contexts
  sunbeam config get
""#)]
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,
    },

    /// User/identity management.
    #[command(long_about = r#"""Manage identities via Kratos.

Provides CRUD operations for users in the Ory Kratos identity system, plus
onboarding and offboarding workflows.

Onboarding creates an identity, sets a random password, and optionally sends
a welcome email with login instructions. Offboarding disables the identity,
revokes all sessions, and marks the user as terminated.

Most commands accept either an email address or a Kratos identity UUID as
the target argument.

EXAMPLES:
  # List all identities
  sunbeam user list

  # Search by email
  sunbeam user list --search admin@sunbeam.pt

  # Create a basic identity
  sunbeam user create alice@sunbeam.pt --name "Alice Smith"

  # Full onboarding with welcome email
  sunbeam user onboard alice@sunbeam.pt --name "Alice Smith" --department Engineering

  # Offboard (disable + revoke)
  sunbeam user offboard alice@sunbeam.pt

  # Set password interactively
  sunbeam user set-password alice@sunbeam.pt
""#)]
    User {
        #[command(subcommand)]
        action: Option<UserAction>,
    },

    /// Authenticate with Sunbeam (OAuth2 login via browser).
    #[command(long_about = r#"""Authenticate with Sunbeam services.

Sunbeam supports two independent authentication flows:

  1. SSO (Hydra OIDC) — used by Planka, Kratos admin UI, Grafana, and other
     services behind the ingress. A browser-based OAuth2 flow obtains an
     access token and refresh token, stored in ~/.sunbeam/config.json.

  2. Gitea — obtains a personal access token for Git operations against
     the internal Gitea instance.

The `login` subcommand runs both flows sequentially. You can also run them
individually with `sso` or `git`.

Tokens are cached locally and refreshed automatically. Use `logout` to clear
all cached tokens. Use `token` to print the current access token for scripts.

EXAMPLES:
  # Log in to both SSO and Gitea (opens browser)
  sunbeam auth login

  # Log in against a specific domain
  sunbeam auth login --domain sunbeam.pt

  # SSO only
  sunbeam auth sso

  # Show current auth status
  sunbeam auth status

  # Print access token for use in scripts / curl headers
  sunbeam auth token

  # Clear all cached tokens
  sunbeam auth logout
""#)]
    Auth {
        #[command(subcommand)]
        action: Option<AuthAction>,
    },

    /// Workflow management — local WFE host, remote wfe-server, and target management.
    #[command(long_about = r#"""Manage WFE (Workflow Engine) instances.

WFE orchestrates complex multi-step operations like `sunbeam up` and
`sunbeam down`. This command lets you inspect, retry, and cancel workflow
instances, as well as manage remote WFE server targets.

Local target (default):
  - Uses an embedded SQLite database at ~/.sunbeam/<context>/workflows.db
  - Supports list, status, retry, cancel, and run

Remote target (-t <name>):
  - Connects to a wfe-server instance over HTTPS
  - Supports server-side workflow registration, execution, log streaming,
    and full-text log search
  - Requires `sunbeam workflow login --name <name> --url <url>` first

EXAMPLES:
  # List local workflow instances
  sunbeam workflow list

  # Show step-by-step status of an instance
  sunbeam workflow status <instance-id>

  # Retry a failed workflow from its last checkpoint
  sunbeam workflow retry <instance-id>

  # Cancel a running workflow
  sunbeam workflow cancel <instance-id>

  # Register a remote target
  sunbeam workflow login --name builds --url https://builds.sunbeam.pt

  # Use remote target for server-side operations
  sunbeam workflow -t builds list
  sunbeam workflow -t builds start --definition deploy --version 1
""#)]
    Workflow {
        /// Workflow target (default: local).
        #[arg(short, long, default_value = "local", global = true)]
        target: String,
        /// Output format.
        #[arg(short, long, value_enum, default_value_t = crate::wfectl::output::OutputFormat::Table, global = true)]
        output: crate::wfectl::output::OutputFormat,
        #[command(subcommand)]
        action: crate::workflows::cmd::WorkflowAction,
    },

    /// Kanban board management.
    #[command(long_about = r#"""Manage Kanban projects, boards, and cards.

Connects to the Sunbeam Kanban backend over gRPC. Most commands require an
active SSO session (`sunbeam auth login`). Public board commands are
unauthenticated.

EXAMPLES:
  # List projects you can access
  sunbeam kanban project list

  # List boards in a project
  sunbeam kanban board list --project proj_xxx

  # Create a card
  sunbeam kanban card create --board board_xxx --title "Fix the thing"

  # Search cards
  sunbeam kanban search "frontend crash"

  # Subscribe to realtime board events
  sunbeam kanban subscribe board board_xxx
""#)]
    Kanban {
        /// Output format.
        #[arg(short, long, value_enum, default_value_t = crate::output::OutputFormat::Table, global = true)]
        output: crate::output::OutputFormat,
        /// Kanban server URL override (default: https://kanban.<domain>).
        #[arg(short, long)]
        url: Option<String>,
        #[command(subcommand)]
        action: crate::kanban::KanbanCommand,
    },

    /// Service operations (deploy, logs, restart, exec, secrets, ...).
    #[command(
        alias = "svc",
        long_about = r#"""Operate on individual services in the cluster.

Provides a curated set of kubectl-adjacent commands scoped to services
(namespaces and deployments). Most commands accept a service name, namespace,
or namespace/name reference.

Key capabilities:
  - status / logs / get    — inspect pods and deployments
  - apply / deploy         — kustomize build + kubectl apply with domain substitution
  - restart / scale / edit — manipulate deployments
  - shell / exec           — interactive debugging inside pods
  - port-forward           — local access to cluster services
  - secrets                — view OpenBao KV secrets for a service
  - seed / verify          — OpenBao credential management and VSO integration tests

The `apply` subcommand is especially important: it runs kustomize build
against the infrastructure directory configured in your context, substitutes
the domain suffix, filters by namespace, and applies with server-side apply.

EXAMPLES:
  # Check status of all pods in a namespace
  sunbeam service status ory

  # Stream logs for a specific deployment
  sunbeam service logs ory/kratos -f

  # Apply a single namespace
  sunbeam service apply ory

  # Apply all namespaces (with confirmation unless --all is used)
  sunbeam service apply --all

  # Dry-run to inspect rendered YAML
  sunbeam service apply ory --dry-run

  # Rolling restart
  sunbeam service restart ory/kratos

  # Interactive shell into a pod
  sunbeam service shell postgres

  # Port-forward Grafana to localhost:3000
  sunbeam service port-forward grafana 3000

  # View secrets stored in OpenBao for a service
  sunbeam service secrets hydra
""#
    )]
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },

    /// VPN management.
    #[command(long_about = r#"""Manage the WireGuard VPN tunnel to the cluster.

Sunbeam integrates Headscale (a self-hosted Tailscale control server) for
VPN access. This provides secure connectivity to cluster services without
exposing them publicly.

Subcommands:
  - status      — show whether the tunnel is active
  - connect     — start the VPN daemon (background by default, --foreground for CLI blocking)
  - disconnect  — stop the tunnel
  - create-key  — mint a new pre-auth key for onboarding another client

When connected, `sunbeam service` and `sunbeam secrets` commands automatically
route through the VPN tunnel. The kube client also rewrites the cluster URL
to a loopback proxy inside the WireGuard trust boundary.

VPN credentials are stored per-context in ~/.sunbeam/config.json.

EXAMPLES:
  # Connect in the background
  sunbeam vpn connect

  # Connect in foreground (useful for debugging)
  sunbeam vpn connect --foreground

  # Check tunnel status
  sunbeam vpn status

  # Disconnect
  sunbeam vpn disconnect

  # Create a reusable key for a new device
  sunbeam vpn create-key --reusable --expiration 30d
""#)]
    Vpn {
        #[command(subcommand)]
        action: VpnAction,
    },

    /// OpenBao secrets engine interaction (KV, transit, generic read/write).
    #[command(long_about = r#"""Interact with the OpenBao secrets engine.

OpenBao is the HashiCorp Vault fork used by Sunbeam for secrets management.
This command provides direct access to KV v2, transit, and generic endpoints.

By default, the command auto-discovers the OpenBao pod in the `openbao` namespace,
opens a port-forward, and reads the root token from the K8s secret
`openbao/openbao-bootstrap-token`. You can override the address and token with --addr and
--token for remote instances.

Subcommands:
  - kv get / put / patch / delete / list  — KV v2 operations
  - transit enable / create-key / read-key / list-keys / delete-key
  - read / write / delete / list          — generic engine access
  - status / init / unseal                — operational commands
  - exec                                  — raw bao CLI passthrough inside the pod

EXAMPLES:
  # Read a KV secret
  sunbeam secrets kv get hydra

  # Write a KV secret
  sunbeam secrets kv put myapp mount=secret foo=bar baz=qux

  # Patch (merge) fields into an existing secret
  sunbeam secrets kv patch myapp foo=new_value

  # Enable transit engine
  sunbeam secrets transit enable transit/sbbb

  # Create an ed25519 transit key
  sunbeam secrets transit create-key transit/sbbb my-key

  # Check seal status
  sunbeam secrets status

  # Unseal with a key
  sunbeam secrets unseal <unseal-key>

  # Raw bao CLI inside the pod
  sunbeam secrets exec policy list
""#)]
    Secrets {
        /// OpenBao address override.
        #[arg(short, long)]
        addr: Option<String>,
        /// Root token override.
        #[arg(short, long)]
        token: Option<String>,
        /// Output format.
        #[arg(short, long, value_enum, default_value_t = crate::output::OutputFormat::Table, global = true)]
        output: crate::output::OutputFormat,
        #[command(subcommand)]
        action: crate::secrets_cli::SecretsAction,
    },

    /// Generate shell completions.
    #[command(long_about = r#"""Generate shell tab-completion scripts.

Outputs a completion script for the specified shell to stdout. Redirect to
the appropriate file for your shell:

  bash:  ~/.bash_completion.d/sunbeam
  zsh:   ~/.zfunc/_sunbeam
  fish:  ~/.config/fish/completions/sunbeam.fish

After installing, restart your shell or source the file.

EXAMPLE:
  sunbeam completions bash > ~/.bash_completion.d/sunbeam
""#)]
    Completions {
        /// Shell to generate completions for.
        #[arg(value_enum)]
        shell: Shell,
    },

    /// Connectivity diagnostics.
    #[command(long_about = r#"""Run connectivity and configuration diagnostics.

Checks that the CLI can reach required services and that configuration is
valid. Reports issues with actionable fixes.

Checks include:
  - Kubernetes API reachability
  - OpenBao seal status and token validity
  - VPN tunnel status (if configured)
  - DNS resolution for the configured domain
  - Infrastructure directory existence and structure

Use this as the first troubleshooting step when something is not working.

EXAMPLE:
  sunbeam doctor
""#)]
    Doctor,

    /// Internal: run the VPN daemon in the foreground.
    #[command(name = "__vpn-daemon", hide = true)]
    VpnDaemon,

    /// Self-update from latest mainline commit.
    #[command(long_about = r#"""Update the Sunbeam CLI to the latest version.

Downloads the latest release artifact from the internal Gitea CI and
replaces the current binary. The update checks the current version against
the latest tagged release.

Requires network access to the artifact registry configured in the update
module. If the binary is managed by a package manager (brew, nix, etc.),
use that instead.

EXAMPLE:
  sunbeam update
""#)]
    Update,

    /// Print version info.
    #[command(long_about = r#"""Print version and build information.

Shows the CLI version, Git commit SHA, build date, and Rust compiler version.
Useful for bug reports and verifying that you are running the expected binary.

EXAMPLE:
  sunbeam version
""#)]
    Version,

    /// Per-project build verbs (alias: proj).
    #[command(
        alias = "proj",
        long_about = r#"""Build, test, and package projects in the workspace.

Sunbeam discovers projects from `sunbeam.workspace.yaml` and `sunbeam.yaml`
files. Each project defines targets (build, test, lint, fmt, package, deploy,
dev, clean, doc) and optional dependencies on other projects.

Commands can run for:
  - The current project only (default)
  - All projects in the workspace (--all), topologically sorted
  - Specific projects (--project foo), optionally including transitive deps
    (--with-deps)

The `package` target builds container images and pushes them to the registry
at `oci.<domain>` or `src.<domain>`.

The `preseed-image` subcommand is a special helper for breaking the Pingora
image pull deadlock on fresh clusters. After `sunbeam project package -p proxy`,
run `sunbeam project preseed-image <ref>` to pull the image on the cluster node,
then `sunbeam service apply ingress` to roll it out.

EXAMPLES:
  # Build the current project
  sunbeam project build

  # Test all projects in dependency order
  sunbeam project test --all

  # Package a specific project
  sunbeam project package -p proxy

  # Run a custom target defined in sunbeam.yaml
  sunbeam project run migrate

  # Show the resolved config for the current project
  sunbeam project info

  # Validate all workspace configs
  sunbeam project check --all
""#
    )]
    Project {
        #[command(subcommand)]
        action: ProjectAction,
    },

    /// Workspace-level operations (alias: ops).
    #[command(
        alias = "ops",
        long_about = r#"""Workspace-level orchestration commands.

Operate across the entire workspace rather than a single project.

Subcommands:
  - compose  — Docker Compose operations for shared local dependencies
               (render, up, down, ps, logs)
  - stack    — snapshot and restore repo SHAs across the workspace
               (list, pin, apply, diff)
  - info     — print resolved workspace configuration
  - repos    — list all repositories in the workspace by bucket

The stack system is useful for pinning a known-good combination of project
versions and later restoring it exactly.

EXAMPLES:
  # Bring up shared local services (Postgres, Redis, etc.)
  sunbeam ops compose up

  # Pin current HEADs as "release-2026-01"
  sunbeam ops stack pin release-2026-01

  # Restore a pinned stack
  sunbeam ops stack apply release-2026-01

  # Diff two stacks
  sunbeam ops stack diff release-2026-01 release-2026-02

  # List all repos
  sunbeam ops repos
""#
    )]
    Operations {
        #[command(subcommand)]
        action: OperationsAction,
    },

    /// Version control — repo tool operations.
    #[command(long_about = r#"""Android repo-style multi-repository operations.

Wraps the repo-rs engine for managing multiple Git repositories via a
manifest. All standard repo subcommands are available:
  init, sync, upload, start, status, diff, rebase, cherry-pick, abandon,
  checkout, branches, forall, grep, manifest, info, list, prune, gc,
  diffmanifests, wipe, selfupdate, smartsync, version, help, overview

EXAMPLES:
  # Initialize a repo checkout
  sunbeam vcs init --manifest-url https://git.sunbeam.pt/manifest.git

  # Sync all projects
  sunbeam vcs sync

  # Show status
  sunbeam vcs status

  # Start a topic branch
  sunbeam vcs start feature-x

  # Upload for review
  sunbeam vcs upload
"""#)]
    Vcs {
        #[command(subcommand)]
        action: VcsAction,
    },
}

impl Verb {
    /// Return a short string identifier for the verb variant (e.g. "up", "version").
    pub fn as_ref_str(&self) -> &'static str {
        match self {
            Verb::Up { .. } => "up",
            Verb::Down { .. } => "down",
            Verb::Config { .. } => "config",
            Verb::User { .. } => "user",
            Verb::Auth { .. } => "auth",
            Verb::Workflow { .. } => "workflow",
            Verb::Kanban { .. } => "kanban",
            Verb::Service { .. } => "service",
            Verb::Vpn { .. } => "vpn",
            Verb::Secrets { .. } => "secrets",
            Verb::Completions { .. } => "completions",
            Verb::Doctor => "doctor",
            Verb::VpnDaemon => "vpn-daemon",
            Verb::Update => "update",
            Verb::Version => "version",
            Verb::Project { .. } => "project",
            Verb::Operations { .. } => "operations",
            Verb::Vcs { .. } => "vcs",
        }
    }
}

/// VPN management subcommands.
#[derive(Subcommand, Debug)]
pub enum VpnAction {
    /// Show VPN tunnel status.
    #[command(long_about = r#"""Show whether the WireGuard VPN tunnel is active.

Reports the local tunnel interface, assigned IP, and last handshake time.
Also shows the Headscale control server reachability.

EXAMPLE:
  sunbeam vpn status
""#)]
    Status,
    /// Connect to the cluster VPN.
    #[command(long_about = r#"""Establish the WireGuard VPN tunnel.

By default, starts the VPN daemon in the background and returns immediately.
Use --foreground to block the CLI and see daemon logs directly.

After connecting, cluster services are reachable via their internal DNS names
and the kube client routes through the tunnel.

EXAMPLES:
  sunbeam vpn connect
  sunbeam vpn connect --foreground
""#)]
    Connect {
        /// Run the daemon in the foreground instead of detaching.
        #[arg(long)]
        foreground: bool,
    },
    /// Disconnect from the cluster VPN.
    #[command(long_about = r#"""Teardown the WireGuard VPN tunnel.

Stops the background VPN daemon and removes the tunnel interface.

EXAMPLE:
  sunbeam vpn disconnect
""#)]
    Disconnect,
    /// Create a new pre-auth key for onboarding a new client.
    #[command(long_about = r#"""Mint a new Headscale pre-authentication key.

Pre-auth keys allow devices to join the VPN without interactive login.
The key is bound to a Headscale user and can optionally be reusable or
ephemeral.

Reusable keys can register multiple devices. Ephemeral keys create nodes
that are automatically removed when the device goes offline.

EXAMPLES:
  # Single-use key for user "sienna"
  sunbeam vpn create-key

  # Reusable key valid for 30 days
  sunbeam vpn create-key --reusable --expiration 30d

  # Ephemeral key for CI runners
  sunbeam vpn create-key --ephemeral --expiration 1h
""#)]
    CreateKey {
        /// Headscale user name the key belongs to (looked up to obtain a numeric ID).
        #[arg(long, default_value = "sunbeam")]
        user: String,
        /// Headscale user numeric ID (skips the /api/v1/user lookup).
        #[arg(long)]
        user_id: Option<u64>,
        /// Make the key reusable across multiple registrations.
        #[arg(long)]
        reusable: bool,
        /// Mark the key (and any node registered with it) ephemeral.
        #[arg(long)]
        ephemeral: bool,
        /// Key lifetime, in human-readable form (e.g. "1h", "30d").
        #[arg(long, default_value = "30d")]
        expiration: String,
    },
}

/// Service operations subcommands.
#[derive(Subcommand, Debug)]
pub enum ServiceAction {
    /// Pod health (optionally scoped).
    #[command(long_about = r#"""Show pod health for a service or namespace.

Without a target, shows all pods across all namespaces. With a target,
scopes to the given namespace or specific pod.

EXAMPLES:
  sunbeam service status
  sunbeam service status ory
  sunbeam service status ory/kratos-7d9f4b8c5-x2v1m
""#)]
    Status {
        /// Service, namespace, or namespace/name.
        target: Option<String>,
    },

    /// kubectl logs for a service.
    #[command(long_about = r#"""Stream or fetch logs for a service.

Follow mode (-f) streams new log lines in real time. Without -f, prints
the current log buffer and exits.

EXAMPLES:
  sunbeam service logs ory/kratos
  sunbeam service logs ory/kratos -f
""#)]
    Logs {
        /// Service or namespace/name.
        target: String,
        /// Stream logs.
        #[arg(short, long)]
        follow: bool,
    },

    /// Raw kubectl get for a pod (ns/name).
    #[command(long_about = r#"""Raw kubectl get output for a pod.

Useful when you need the full pod spec, status, or metadata in YAML, JSON,
or wide table format.

EXAMPLES:
  sunbeam service get ory/kratos-7d9f4b8c5-x2v1m
  sunbeam service get ory/kratos-7d9f4b8c5-x2v1m -o json
""#)]
    Get {
        /// Service or namespace/name.
        target: String,
        /// Output format.
        #[arg(short, long, default_value = "yaml", value_parser = ["yaml", "json", "wide"])]
        output: String,
    },

    /// Rolling restart of services.
    #[command(long_about = r#"""Perform a rolling restart of a deployment.

Triggers a rolling update by patching the deployment's pod template with a
restart annotation. Does not change the container image.

Without a target, restarts all services. With a target, scopes to the given
namespace or specific deployment.

EXAMPLES:
  sunbeam service restart ory/kratos
  sunbeam service restart ory
""#)]
    Restart {
        /// Service, namespace, or namespace/name.
        target: Option<String>,
    },

    /// Functional service health checks.
    #[command(long_about = r#"""Run functional health checks against services.

Performs application-level checks (HTTP endpoints, database connectivity,
etc.) rather than just pod status. Reports detailed failure reasons.

EXAMPLES:
  sunbeam service check
  sunbeam service check ory
""#)]
    Check {
        /// Service, namespace, or namespace/name.
        target: Option<String>,
    },

    /// Deploy service(s) — apply manifests + rollout restart.
    #[command(long_about = r#"""Deploy one or more services.

Applies manifests via kustomize build + kubectl apply, then triggers a
rolling restart. This is a convenience wrapper around `apply` + `restart`.

Use --all to deploy every namespace. Use --profile to apply a preset
configuration of skips and overrides.

EXAMPLES:
  sunbeam service deploy ory
  sunbeam service deploy --all
  sunbeam service deploy hydra --profile minimal
""#)]
    Deploy {
        /// Service name, category, or namespace (e.g. "hydra", "auth", "ory").
        /// Use --all for everything.
        target: Option<String>,
        /// Deploy all services.
        #[arg(long)]
        all: bool,
        /// Apply a named profile (shortcuts, skips, overrides).
        #[arg(long)]
        profile: Option<String>,
    },

    /// List all available services that can be applied.
    #[command(
        long_about = r#"""List services discovered in the infrastructure directory.

Shows namespace, service name, kind, and whether the resource is currently
enabled or disabled by profile rules.

EXAMPLE:
  sunbeam service list
  sunbeam service list -o json
""#
    )]
    List {
        /// Output format.
        #[arg(short, long, value_enum, default_value_t = crate::output::OutputFormat::Table)]
        format: crate::output::OutputFormat,
    },

    /// kustomize build + domain subst + kubectl apply.
    #[command(long_about = r#"""Apply Kubernetes manifests for a namespace.

Runs kustomize build against the infrastructure directory configured in the
active context, substitutes the domain suffix, applies manifest overrides
(--set, --disable, --enable), and applies with server-side apply.

The infrastructure directory is NOT a flag on this command — it's a
per-context config. Set it with:
  sunbeam config set --context-name <name> --infra-dir <path>

Use --dry-run to inspect the rendered YAML without applying.
Use --all to apply every namespace (with confirmation unless --all is used
in non-interactive contexts).

EXAMPLES:
  sunbeam service apply ory
  sunbeam service apply ory --dry-run
  sunbeam service apply --all
  sunbeam service apply ory --set deployment/ory/kratos/spec/replicas=3
""#)]
    Apply {
        /// Limit apply to one namespace.
        namespace: Option<String>,
        /// Apply all namespaces without confirmation.
        #[arg(long = "all")]
        apply_all: bool,
        /// Domain suffix override (e.g. sunbeam.pt). Defaults to the active
        /// context's `domain`; set via `sunbeam config set --domain`.
        #[arg(long, default_value = "")]
        domain: String,
        /// ACME email override for cert-manager. Defaults to the active
        /// context's `acme-email`; set via `sunbeam config set --acme-email`.
        #[arg(long, default_value = "")]
        email: String,
        /// Print the post-substitution YAML without calling kubectl apply.
        #[arg(long)]
        dry_run: bool,
        /// Override a manifest field (kind/namespace/name/field/path=value).
        #[arg(long = "set")]
        set: Vec<String>,
        /// Disable a resource or pattern.
        #[arg(long)]
        disable: Vec<String>,
        /// Re-enable a resource or pattern.
        #[arg(long)]
        enable: Vec<String>,
        /// Apply a named profile (shortcuts, skips, overrides).
        #[arg(long)]
        profile: Option<String>,
    },

    /// Generate/store all credentials in OpenBao.
    #[command(long_about = r#"""Seed OpenBao with credentials for all services.

Generates random secrets (passwords, salts, tokens) for every service in the
stack and writes them to OpenBao KV paths. This is normally done automatically
by `sunbeam up`, but can be run standalone after a cluster reset or when
adding new services.

EXAMPLE:
  sunbeam service seed
""#)]
    Seed,

    /// E2E VSO + OpenBao integration test.
    #[command(long_about = r#"""Verify Vault Secrets Operator integration.

Creates a test VaultStaticSecret and VaultAuth resource, waits for VSO to
sync the secret into a Kubernetes Secret, and validates the value. This
confirms that the OpenBao → VSO → K8s secret pipeline is working end-to-end.

EXAMPLE:
  sunbeam service verify
""#)]
    Verify,

    /// View or get secrets for a service from OpenBao.
    #[command(long_about = r#"""View secrets stored in OpenBao for a service.

Without a subcommand, lists all keys in the service's KV path.
With `get <key>`, prints the specific field value.

EXAMPLES:
  sunbeam service secrets hydra
  sunbeam service secrets hydra get secretsSystem
""#)]
    Secrets {
        /// Service name (e.g. "hydra").
        service: String,
        #[command(subcommand)]
        action: Option<SecretsAction>,
    },

    /// Interactive shell into a service pod.
    #[command(long_about = r#"""Open an interactive shell in a service pod.

Uses kubectl exec with /bin/sh. The service name is resolved to a running
pod automatically.

EXAMPLE:
  sunbeam service shell postgres
  sunbeam service shell gitea
""#)]
    Shell {
        /// Service name (e.g. "postgres", "gitea").
        service: String,
    },

    /// Describe a service (kubectl describe on its deployment).
    #[command(long_about = r#"""kubectl describe for a service deployment.

Shows events, conditions, replica status, and resource usage.

EXAMPLE:
  sunbeam service describe hydra
""#)]
    Describe {
        /// Service name.
        service: String,
    },

    /// Exec into a service pod.
    #[command(long_about = r#"""Execute a command in a service pod.

The command and arguments are passed directly to kubectl exec. Use --container
to target a specific container in multi-container pods.

EXAMPLES:
  sunbeam service exec postgres -- psql -U kratos
  sunbeam service exec hydra --container sidecar -- ls /var/log
""#)]
    Exec {
        /// Service name.
        service: String,
        /// Container name (optional).
        #[arg(short, long)]
        container: Option<String>,
        /// Command and arguments to run.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },

    /// Port-forward to a service pod.
    #[command(long_about = r#"""Forward local ports to a service pod.

Accepts standard kubectl port-forward syntax: "local:remote" or just "port"
for symmetric mapping. Blocks until interrupted (Ctrl-C).

EXAMPLES:
  sunbeam service port-forward grafana 3000
  sunbeam service port-forward hydra 4445:4445
""#)]
    PortForward {
        /// Service name.
        service: String,
        /// Port mapping (e.g. "8080:80", "8080").
        ports: Vec<String>,
    },

    /// Scale a service deployment.
    #[command(
        long_about = r#"""Scale a deployment to the specified number of replicas.

EXAMPLE:
  sunbeam service scale hydra 3
""#
    )]
    Scale {
        /// Service name.
        service: String,
        /// Number of replicas.
        replicas: u32,
    },

    /// Show resource usage (CPU/memory) for a service's pods.
    #[command(long_about = r#"""Show CPU and memory usage for a service's pods.

Uses kubectl top. Requires the metrics-server to be running in the cluster.

EXAMPLE:
  sunbeam service top hydra
""#)]
    Top {
        /// Service name.
        service: String,
    },

    /// Edit a service's deployment manifest in-cluster.
    #[command(long_about = r#"""Edit a deployment in-cluster with $EDITOR.

Opens the live deployment manifest in your default editor. Changes are
applied immediately. This is a kubectl edit wrapper.

EXAMPLE:
  sunbeam service edit hydra
""#)]
    Edit {
        /// Service name.
        service: String,
    },

    /// Delete a Job by namespace/name (workaround for k8s Job immutability).
    #[command(long_about = r#"""Delete a Kubernetes Job.

Kubernetes Jobs are immutable — you cannot update their spec. When a Job's
container spec changes, you must delete and recreate it. This command is a
convenience wrapper for that operation.

EXAMPLE:
  sunbeam service delete-job build/migrate-2026-01-15
""#)]
    DeleteJob {
        /// Job reference in the form <namespace>/<name>.
        target: String,
    },
}

/// Secret field retrieval subcommands.
#[derive(Subcommand, Debug)]
pub enum SecretsAction {
    /// Get a specific secret field value.
    #[command(
        long_about = r#"""Get a specific field from a service's OpenBao KV secret.

EXAMPLE:
  sunbeam service secrets hydra get secretsSystem
""#
    )]
    Get {
        /// Field name within the service's KV path.
        key: String,
    },
}

/// Authentication subcommands (login, logout, token).
#[derive(Subcommand, Debug)]
pub enum AuthAction {
    /// Log in to both SSO and Gitea.
    #[command(long_about = r#"""Run both SSO and Gitea login flows sequentially.

Opens a browser for Hydra OIDC (SSO) and then requests a Gitea personal
access token. Both tokens are cached in ~/.sunbeam/config.json.

Use --domain to authenticate against a specific domain. If omitted, the
active context's domain is used.

EXAMPLE:
  sunbeam auth login
  sunbeam auth login --domain staging.sunbeam.pt
""#)]
    Login {
        /// Domain to authenticate against (e.g. sunbeam.pt).
        #[arg(long)]
        domain: Option<String>,
        /// Use the OAuth2 Device Authorization Grant (headless login).
        #[arg(long)]
        device: bool,
    },
    /// Log in to SSO only (Hydra OIDC — for Planka, identity management).
    #[command(long_about = r#"""Log in to SSO only.

Useful when you only need access to SSO-protected services (Grafana, Planka,
Kratos admin UI) and do not need Git access.

Use --device for headless environments (RFC 8628 device code flow).

EXAMPLE:
  sunbeam auth sso
  sunbeam auth sso --device
""#)]
    Sso {
        /// Domain to authenticate against.
        #[arg(long)]
        domain: Option<String>,
        /// Use the OAuth2 Device Authorization Grant (headless login).
        #[arg(long)]
        device: bool,
    },
    /// Log in with a device code (headless OAuth2 Device Authorization Grant).
    #[command(long_about = r#"""Log in using a device code.

For headless environments or when a browser cannot be opened. The CLI prints a
URL and a user code; authorize the device in a browser, and the CLI polls for
tokens. After SSO completes, run `sunbeam auth git` if you also need Git access.

EXAMPLE:
  sunbeam auth device
  sunbeam auth device --domain staging.sunbeam.pt
""#)]
    Device {
        /// Domain to authenticate against.
        #[arg(long)]
        domain: Option<String>,
    },
    /// Log in to Gitea only (personal access token).
    #[command(long_about = r#"""Log in to Gitea only.

Useful for CI pipelines or machines that only need Git access, not SSO.

EXAMPLE:
  sunbeam auth git
""#)]
    Git {
        /// Domain to authenticate against.
        #[arg(long)]
        domain: Option<String>,
    },
    /// Log out (remove all cached tokens).
    #[command(long_about = r#"""Clear all cached authentication tokens.

Removes SSO access tokens, refresh tokens, and Gitea personal access tokens
from ~/.sunbeam/config.json. Does not delete identities or server-side sessions.

EXAMPLE:
  sunbeam auth logout
""#)]
    Logout,
    /// Show current authentication status.
    #[command(long_about = r#"""Show which services you are authenticated with.

Reports whether a valid SSO token and/or Gitea token is present, when it
expires, and which domain it belongs to.

EXAMPLE:
  sunbeam auth status
""#)]
    Status,
    /// Print the current access token (for use in scripts and MCP headers).
    #[command(long_about = r#"""Print the current SSO access token to stdout.

Useful for scripting and API clients that need a Bearer token. The token is
printed with no additional formatting — pipe or copy as needed.

EXAMPLE:
  curl -H "Authorization: Bearer $(sunbeam auth token)" https://api.sunbeam.pt/...
""#)]
    Token,
}

/// Configuration management subcommands.
#[derive(Subcommand, Debug)]
pub enum ConfigAction {
    /// Set configuration values for the current context.
    #[command(long_about = r#"""Set fields on a Sunbeam context.

All fields are optional — omitting a field leaves it unchanged. Empty string
is treated as "don't change" (not as an actual empty value).

If --context-name is omitted, the currently active context is modified.
If the named context does not exist, it is created automatically.

EXAMPLES:
  # Set domain and infra-dir on active context
  sunbeam config set --domain sunbeam.pt --infra-dir ~/code/infra

  # Set kube-context on a specific context
  sunbeam config set --context-name staging --kube-context k3s-staging
""#)]
    Set {
        /// Domain suffix (e.g. sunbeam.pt).
        #[arg(long, default_value = "")]
        domain: String,
        /// Infrastructure directory root.
        #[arg(long, default_value = "")]
        infra_dir: String,
        /// ACME email for Let's Encrypt certificates.
        #[arg(long, default_value = "")]
        acme_email: String,
        /// kubectl context name this Sunbeam context targets (must exist in
        /// your kubeconfig). Required for any command that touches the cluster.
        #[arg(long, default_value = "")]
        kube_context: String,
        /// Context name to configure (default: current context).
        #[arg(long, default_value = "")]
        context_name: String,
    },
    /// Get current configuration.
    #[command(long_about = r#"""Print all contexts and highlight the active one.

Shows domain, kube-context, infra-dir, and acme-email for each context.

EXAMPLE:
  sunbeam config get
""#)]
    Get,
    /// Clear configuration.
    #[command(long_about = r#"""Delete the entire ~/.sunbeam/config.json file.

Use with caution. This removes all contexts, profiles, and cached tokens.

EXAMPLE:
  sunbeam config clear
""#)]
    Clear,
    /// Switch the active context.
    #[command(long_about = r#"""Switch the active context.

If the context does not exist, it is created with default values. Subsequent
commands will use this context's domain, kube-context, and infra-dir.

EXAMPLE:
  sunbeam config use-context production
""#)]
    UseContext {
        /// Context name to switch to.
        name: String,
    },
}

/// User/identity management subcommands.
#[derive(Subcommand, Debug)]
pub enum UserAction {
    /// List identities.
    #[command(long_about = r#"""List all Kratos identities.

Output is paginated by the Kratos admin API. Use --search to filter by email
substring.

EXAMPLES:
  sunbeam user list
  sunbeam user list --search admin
""#)]
    List {
        /// Filter by email.
        #[arg(long, default_value = "")]
        search: String,
    },
    /// Get identity by email or ID.
    #[command(long_about = r#"""Show full identity details.

Accepts either an email address or a Kratos identity UUID. Resolves email to
ID internally when needed.

EXAMPLE:
  sunbeam user get admin@sunbeam.pt
  sunbeam user get 7c8f3e2a-...
""#)]
    Get {
        /// Email or identity ID.
        target: String,
    },
    /// Create identity.
    #[command(long_about = r#"""Create a bare identity.

For a full onboarding workflow (welcome email, password generation), use
`onboard` instead.

EXAMPLE:
  sunbeam user create alice@sunbeam.pt --name "Alice" --schema default
""#)]
    Create {
        /// Email address.
        email: String,
        /// Display name.
        #[arg(long, default_value = "")]
        name: String,
        /// Schema ID.
        #[arg(long, default_value = "default")]
        schema: String,
    },
    /// Delete identity.
    #[command(long_about = r#"""Permanently delete an identity.

This is irreversible. Consider `offboard` for a reversible lockout.

EXAMPLE:
  sunbeam user delete alice@sunbeam.pt
""#)]
    Delete {
        /// Email or identity ID.
        target: String,
    },
    /// Generate recovery link.
    #[command(long_about = r#"""Generate a recovery link for an identity.

The link can be sent to the user to regain account access.

EXAMPLE:
  sunbeam user recover alice@sunbeam.pt
""#)]
    Recover {
        /// Email or identity ID.
        target: String,
    },
    /// Disable identity + revoke sessions (lockout).
    #[command(long_about = r#"""Disable an identity and revoke all active sessions.

The identity data is preserved. Use `enable` to reactivate.

EXAMPLE:
  sunbeam user disable alice@sunbeam.pt
""#)]
    Disable {
        /// Email or identity ID.
        target: String,
    },
    /// Re-enable a disabled identity.
    #[command(long_about = r#"""Re-enable a previously disabled identity.

Does not reset the password or send any notification.

EXAMPLE:
  sunbeam user enable alice@sunbeam.pt
""#)]
    Enable {
        /// Email or identity ID.
        target: String,
    },
    /// Set password for an identity.
    #[command(long_about = r#"""Set or reset a user's password.

If the password argument is omitted, it is read interactively from stdin
(without echo). Useful for automation when piped:

  echo 'newpass' | sunbeam user set-password alice@sunbeam.pt

EXAMPLE:
  sunbeam user set-password alice@sunbeam.pt hunter2
""#)]
    SetPassword {
        /// Email or identity ID.
        target: String,
        /// New password. If omitted, reads from stdin.
        password: Option<String>,
    },
    /// Onboard new user (create + welcome email).
    #[command(long_about = r#"""Complete onboarding workflow for a new team member.

Creates the identity, generates a random password, and sends a welcome email
with login instructions. Supports rich metadata like department, manager, and
hire date for HR integrations.

Use --no-email to skip the welcome email (e.g. when provisioning accounts
ahead of a start date). Use --notify to send to a different address than the
identity email.

EXAMPLES:
  # Basic onboarding
  sunbeam user onboard alice@sunbeam.pt --name "Alice Smith"

  # Full HR metadata
  sunbeam user onboard alice@sunbeam.pt \
    --name "Alice Smith" \
    --department Engineering \
    --job-title "Senior Developer" \
    --hire-date 2026-01-15 \
    --manager bob@sunbeam.pt
""#)]
    Onboard {
        /// Email address.
        email: String,
        /// Display name (First Last).
        #[arg(long, default_value = "")]
        name: String,
        /// Schema ID.
        #[arg(long, default_value = "employee")]
        schema: String,
        /// Skip sending welcome email.
        #[arg(long)]
        no_email: bool,
        /// Send welcome email to this address instead.
        #[arg(long, default_value = "")]
        notify: String,
        /// Job title.
        #[arg(long, default_value = "")]
        job_title: String,
        /// Department.
        #[arg(long, default_value = "")]
        department: String,
        /// Office location.
        #[arg(long, default_value = "")]
        office_location: String,
        /// Hire date (YYYY-MM-DD).
        #[arg(long, default_value = "", value_parser = validate_date)]
        hire_date: String,
        /// Manager name or email.
        #[arg(long, default_value = "")]
        manager: String,
    },
    /// Offboard user (disable + revoke all).
    #[command(long_about = r#"""Offboard a user immediately.

Disables the identity, revokes all sessions, and marks the user as terminated.
This is the recommended offboarding command. It is reversible with `enable`.

EXAMPLE:
  sunbeam user offboard alice@sunbeam.pt
""#)]
    Offboard {
        /// Email or identity ID.
        target: String,
    },
}

/// Per-project build and lifecycle subcommands.
#[derive(Subcommand, Debug)]
pub enum ProjectAction {
    /// Build the project.
    #[command(long_about = r#"""Build the current project or selected projects.

Runs the `build` target defined in `sunbeam.yaml`. Respects the dependency
graph — if --with-deps is used, dependencies are built first.

EXAMPLES:
  sunbeam project build
  sunbeam project build --all
  sunbeam project build -p proxy --with-deps
""#)]
    Build(ProjectRunArgs),
    /// Run tests.
    #[command(
        long_about = r#"""Run tests for the current project or selected projects.

Runs the `test` target defined in `sunbeam.yaml`.

EXAMPLES:
  sunbeam project test
  sunbeam project test --all
""#
    )]
    Test(ProjectRunArgs),
    /// Lint.
    #[command(
        long_about = r#"""Run the linter for the current project or selected projects.

Runs the `lint` target defined in `sunbeam.yaml`.

EXAMPLES:
  sunbeam project lint
  sunbeam project lint --all
""#
    )]
    Lint(ProjectRunArgs),
    /// Format.
    #[command(
        long_about = r#"""Format source code for the current project or selected projects.

Runs the `fmt` target defined in `sunbeam.yaml`.

EXAMPLES:
  sunbeam project fmt
  sunbeam project fmt --all
""#
    )]
    Fmt(ProjectRunArgs),
    /// Package (container image, tarball, etc.).
    #[command(long_about = r#"""Package the project into a distributable artifact.

For most services this builds a container image and pushes it to the registry
at `oci.<domain>` or `src.<domain>`. The image tag is derived from the Git
commit SHA.

EXAMPLES:
  sunbeam project package
  sunbeam project package -p proxy
  sunbeam project package --all
""#)]
    Package(ProjectRunArgs),
    /// Deploy.
    #[command(long_about = r#"""Deploy the current project.

Runs the `deploy` target defined in `sunbeam.yaml`. This usually applies
manifests and triggers a rolling restart.

EXAMPLES:
  sunbeam project deploy
  sunbeam project deploy -p proxy
""#)]
    Deploy(ProjectRunArgs),
    /// Run dev server.
    #[command(long_about = r#"""Run the development server for the current project.

Runs the `dev` target defined in `sunbeam.yaml`. This typically starts a
hot-reload server with local dependencies.

EXAMPLE:
  sunbeam project dev
""#)]
    Dev(ProjectRunArgs),
    /// Clean build artifacts.
    #[command(long_about = r#"""Remove build artifacts for the current project.

Runs the `clean` target defined in `sunbeam.yaml`.

EXAMPLES:
  sunbeam project clean
  sunbeam project clean --all
""#)]
    Clean(ProjectRunArgs),
    /// Generate docs.
    #[command(long_about = r#"""Generate documentation for the current project.

Runs the `doc` target defined in `sunbeam.yaml`.

EXAMPLES:
  sunbeam project doc
  sunbeam project doc --all
""#)]
    Doc(ProjectRunArgs),
    /// Print resolved project config.
    #[command(long_about = r#"""Print the resolved sunbeam.yaml configuration.

Shows merged config, inherited workspace values, computed fields, and the
final target definitions for the current project.

EXAMPLE:
  sunbeam project info
""#)]
    Info,
    /// Print topologically-sorted build order for a verb.
    #[command(
        long_about = r#"""Show the build order for a verb across the workspace.

Respects project dependencies (`deps.projects` in sunbeam.yaml) and prints
the topological groups. Useful for understanding parallelism limits.

EXAMPLE:
  sunbeam project order build
  sunbeam project order package
""#
    )]
    Order {
        #[arg(default_value = "build")]
        verb: String,
    },
    /// Run a custom verb defined in this project's `targets`.
    #[command(long_about = r#"""Run a custom target defined in sunbeam.yaml.

The verb must exist in the `targets:` section of the project's config.
Custom targets are useful for project-specific operations like database
migrations, code generation, or integration tests.

EXAMPLE:
  sunbeam project run migrate
  sunbeam project run integration-tests
""#)]
    Run {
        /// Verb name (must exist in `targets:` of the autodiscovered sunbeam.yaml).
        verb: String,
        #[command(flatten)]
        args: ProjectRunArgs,
    },
    /// Print the dependency DAG as a tree.
    #[command(long_about = r#"""Render the project dependency graph.

Prints a tree showing which projects depend on which. With --all, renders
the tree for every owned project in the workspace.

EXAMPLES:
  sunbeam project graph
  sunbeam project graph --all
""#)]
    Graph {
        /// Render every owned project's tree (otherwise just the current project).
        #[arg(long)]
        all: bool,
    },
    /// Validate sunbeam.yaml — checks workspace refs, dep cycles, and reports issues.
    #[command(long_about = r#"""Validate sunbeam.yaml files.

Checks for:
  - Missing workspace references
  - Dependency cycles
  - Invalid target definitions
  - Missing required fields

With --all, validates every owned project. Otherwise validates only the
current project.

EXAMPLES:
  sunbeam project check
  sunbeam project check --all
""#)]
    Check {
        /// Validate every owned project (otherwise just the current project).
        #[arg(long)]
        all: bool,
    },
    /// Pre-pull the proxy image on the cluster node to break the
    /// Pingora IfNotPresent + Recreate deadlock, then bump the
    /// kustomization newTag.
    #[command(long_about = r#"""Pre-pull a container image on the cluster node.

On fresh clusters, the Pingora ingress controller can deadlock because its
image pull policy is IfNotPresent and the registry is behind the ingress
itself. This command pulls the image directly onto the node via a Job, then
patches the kustomization newTag so the deployment can roll out.

Workflow:
  1. sunbeam project package -p proxy
  2. sunbeam project preseed-image src.sunbeam.pt/studio/proxy:abc1234
  3. sunbeam service apply ingress

EXAMPLE:
  sunbeam project preseed-image src.sunbeam.pt/studio/proxy:abc1234
""#)]
    PreseedImage {
        /// Full image reference to pull (e.g. src.sunbeam.pt/studio/proxy:abc1234).
        image_ref: String,
        /// Seconds to wait for the puller Job to complete.
        #[arg(long, default_value_t = 300)]
        timeout: u64,
    },
}

#[derive(clap::Args, Debug, Clone, Default)]
/// Projectrunargs.
pub struct ProjectRunArgs {
    /// Run for all projects in the workspace (topo-ordered). If omitted, runs
    /// for the current project only.
    #[arg(long)]
    pub all: bool,
    /// Run only for the named project(s). Implies workspace mode.
    #[arg(long = "project", short = 'p')]
    pub projects: Vec<String>,
    /// When `--project foo` is used, also run foo's transitive deps first.
    #[arg(long)]
    pub with_deps: bool,
    /// Cap parallelism within each topo group (default: unbounded).
    #[arg(long)]
    pub jobs: Option<usize>,
    /// Print commands before running.
    #[arg(long)]
    pub echo: bool,
    /// Print what would run, don't execute.
    #[arg(long)]
    pub dry_run: bool,
}

/// Workspace operations subcommands.
#[derive(Subcommand, Debug)]
pub enum OperationsAction {
    /// Docker compose operations.
    Compose {
        #[command(subcommand)]
        action: ComposeAction,
    },
    /// Stack snapshot operations.
    Stack {
        #[command(subcommand)]
        action: StackAction,
    },
    /// Print resolved workspace config.
    #[command(long_about = r#"""Print the resolved workspace configuration.

Shows the merged sunbeam.workspace.yaml with all inherited values,
project discovery results, and bucket assignments.

EXAMPLE:
  sunbeam ops info
""#)]
    Info,
    /// List all repos in the workspace (by bucket).
    #[command(long_about = r#"""List all repositories in the workspace.

Shows each repo's URL, local path, bucket (owned / fork / upstream), and
current branch.

EXAMPLE:
  sunbeam ops repos
""#)]
    Repos,
}

/// Repo tool subcommands.
#[derive(Subcommand, Debug)]
#[command(disable_help_subcommand = true)]
pub enum VcsAction {
    /// Initialize a repo client checkout.
    Init(repo_rs_cmd::init::InitArgs),
    /// Update working tree to the latest revision.
    Sync(repo_rs_cmd::sync::SyncArgs),
    /// Upload changes for code review.
    Upload(repo_rs_cmd::upload::UploadArgs),
    /// Download changes from the server.
    Download(repo_rs_cmd::download::DownloadArgs),
    /// Start a new branch for development.
    Start(repo_rs_cmd::start::StartArgs),
    /// Show the working tree status.
    Status(repo_rs_cmd::status::StatusArgs),
    /// Show changes between commits, commit and working tree, etc.
    Diff(repo_rs_cmd::diff::DiffArgs),
    /// Stage files for upload.
    Stage(repo_rs_cmd::stage::StageArgs),
    /// Rebase local branches.
    Rebase(repo_rs_cmd::rebase::RebaseArgs),
    /// Cherry-pick a change.
    CherryPick(repo_rs_cmd::cherry_pick::CherryPickArgs),
    /// Abandon a topic branch.
    Abandon(repo_rs_cmd::abandon::AbandonArgs),
    /// Checkout a branch.
    Checkout(repo_rs_cmd::checkout::CheckoutArgs),
    /// List, create, or delete branches.
    Branches(repo_rs_cmd::branches::BranchesArgs),
    /// Run a shell command in each project.
    Forall(repo_rs_cmd::forall::ForallArgs),
    /// Search across projects.
    Grep(repo_rs_cmd::grep::GrepArgs),
    /// Manifest inspection and comparison.
    Manifest(repo_rs_cmd::manifest::ManifestArgs),
    /// Display info about a project.
    Info(repo_rs_cmd::info::InfoArgs),
    /// List projects.
    List(repo_rs_cmd::list::ListArgs),
    /// Prune branches.
    Prune(repo_rs_cmd::prune::PruneArgs),
    /// Garbage collection.
    Gc(repo_rs_cmd::gc::GcArgs),
    /// Diff manifests.
    Diffmanifests(repo_rs_cmd::diffmanifests::DiffManifestsArgs),
    /// Wipe a project.
    Wipe(repo_rs_cmd::wipe::WipeArgs),
    /// Update the repo tool itself.
    Selfupdate(repo_rs_cmd::selfupdate::SelfUpdateArgs),
    /// Smart sync.
    Smartsync(repo_rs_cmd::smartsync::SmartsyncArgs),
    /// Display the version of repo.
    Version(repo_rs_cmd::version::VersionArgs),
    /// Display detailed help.
    Help(repo_rs_cmd::help::HelpArgs),
    /// Show project overview.
    Overview(repo_rs_cmd::overview::OverviewArgs),
}
/// Docker Compose subcommands.
#[derive(Subcommand, Debug)]
pub enum ComposeAction {
    /// Generate and materialize docker-compose.yaml under .sunbeam/compose/.
    #[command(long_about = r#"""Render docker-compose.yaml from templates.

Generates a docker-compose file under .sunbeam/compose/ based on the
workspace configuration. Does not start any containers.

EXAMPLE:
  sunbeam ops compose render
""#)]
    Render,
    /// Bring up shared services.
    #[command(long_about = r#"""Start shared local services with Docker Compose.

Brings up Postgres, Redis, and any other shared dependencies defined in the
workspace compose configuration. These are used for local development when
you don't want to target the full cluster.

EXAMPLES:
  sunbeam ops compose up
  sunbeam ops compose up postgres redis
  sunbeam ops compose up --no-wait
""#)]
    Up {
        /// Only bring up the named services (default: all).
        services: Vec<String>,
        /// Wait for healthy.
        #[arg(long, default_value_t = true)]
        wait: bool,
    },
    /// Tear down shared services.
    #[command(long_about = r#"""Stop shared local services.

Stops and removes containers created by `sunbeam ops compose up`.
Use --volumes to also remove named volumes (destructive).

EXAMPLES:
  sunbeam ops compose down
  sunbeam ops compose down --volumes
""#)]
    Down {
        /// Also remove volumes.
        #[arg(long)]
        volumes: bool,
    },
    /// List running services.
    #[command(long_about = r#"""List running compose services.

Shows container names, ports, and health status.

EXAMPLE:
  sunbeam ops compose ps
""#)]
    Ps,
    /// Tail logs for a service.
    #[command(long_about = r#"""Stream or fetch logs for a compose service.

EXAMPLES:
  sunbeam ops compose logs postgres
  sunbeam ops compose logs postgres -f
""#)]
    Logs {
        service: String,
        #[arg(short, long)]
        follow: bool,
    },
}

/// Stack snapshot subcommands.
#[derive(Subcommand, Debug)]
pub enum StackAction {
    /// List pinned stacks.
    #[command(long_about = r#"""List all pinned stack snapshots.

Shows name, creation date, description, and number of projects.

EXAMPLE:
  sunbeam ops stack list
""#)]
    List,
    /// Pin current HEAD SHAs into a named stack.
    #[command(long_about = r#"""Save the current workspace state as a named stack.

Records the current Git HEAD SHA for every owned project. Later, you can
restore exactly this combination with `apply`.

Use --description to add a human-readable note. Use positional projects
to pin only specific repos.

EXAMPLES:
  sunbeam ops stack pin release-2026-01
  sunbeam ops stack pin hotfix --description "Emergency security patches"
  sunbeam ops stack pin partial proxy api
""#)]
    Pin {
        name: String,
        /// Only pin the named projects (default: all owned).
        projects: Vec<String>,
        #[arg(long)]
        description: Option<String>,
    },
    /// Checkout the SHAs recorded in a stack.
    #[command(long_about = r#"""Restore a pinned stack.

Checks out the recorded SHAs in every project. Fails if a repo has uncommitted
changes that would be overwritten.

EXAMPLE:
  sunbeam ops stack apply release-2026-01
""#)]
    Apply { name: String },
    /// Diff two stacks (or a stack vs current HEAD).
    #[command(long_about = r#"""Compare two stack snapshots.

Shows which projects differ between the two stacks. If right is omitted,
compares the left stack against the current HEAD.

EXAMPLES:
  sunbeam ops stack diff release-2026-01 release-2026-02
  sunbeam ops stack diff release-2026-01
""#)]
    Diff { left: String, right: Option<String> },
}

fn validate_date(s: &str) -> std::result::Result<String, String> {
    if s.is_empty() {
        return Ok(s.to_string());
    }
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map(|_| s.to_string())
        .map_err(|_| format!("Invalid date: '{s}' (expected YYYY-MM-DD)"))
}

/// Main dispatch function — parse CLI args and route to subcommands.
#[tracing::instrument(skip(logger, cli), fields(verb = tracing::field::Empty))]
pub async fn dispatch(logger: &crate::logger::Logger, cli: Cli) -> Result<()> {
    let verb_name = cli.verb.as_ref().map(|v| v.as_ref_str());
    tracing::Span::current().record("verb", &verb_name.unwrap_or("none"));
    debug!(logger, "cli dispatch", verb = format!("{:?}", cli.verb));

    // Resolve the active context from config + CLI flags (like kubectl).
    // `--domain` / `--email` are Option<String>: `None` means "don't override",
    // which preserves the config value. The previous empty-string default
    // could still look "present" to sloppy call sites and was load-bearing
    // for note #3's bug report — keeping an Option here makes the intent
    // explicit.
    let config = crate::config::load_config();
    let mut active = crate::config::resolve_context(
        &config,
        "",
        cli.context.as_deref(),
        cli.domain.as_deref().unwrap_or(""),
    );

    // Resolve domain once at the CLI boundary. If the context has no domain,
    // discover it from cluster state (gitea-inline-config secret or Lima IP).
    if active.domain.is_empty() {
        match crate::kube::get_domain().await {
            Ok(d) if !d.is_empty() => active.domain = d,
            _ => {}
        }
    }

    // Resolve ACME email once at the CLI boundary.
    if active.acme_email.is_empty() {
        active.acme_email = "ops@sunbeam.pt".to_string();
    }

    // Thread the active Sunbeam context's kube-context into the shared kube
    // client. An empty value here means the user picked a context that has
    // no `kube-context` configured — we stash the empty string and let the
    // first kube operation surface a clear error pointing at
    // `sunbeam config set --kube-context <kctx>`. The previous behaviour
    // silently fell back to "sunbeam", which routed applies at a wrong
    // cluster (see notes/sunbeam-cli-context-gotchas.md).
    crate::kube::set_context(&active.kube_context);

    // Store active context globally for other modules to read
    crate::config::set_active_context(active);

    match cli.verb {
        None => {
            // Print help via clap
            use clap::CommandFactory;
            Cli::command().print_help()?;
            println!();
            Ok(())
        }

        Some(Verb::Down {
            yes,
            infra,
            keep_data,
            use_lima,
            profile,
        }) => {
            // Confirmation prompt (kept outside the workflow so the workflow
            // itself is non-interactive and fully automatable).
            if !yes {
                let mut to_delete: Vec<&str> = crate::down::APP_NAMESPACES.to_vec();
                if infra {
                    to_delete.extend(crate::down::INFRA_NAMESPACES);
                }
                if keep_data {
                    to_delete.retain(|&ns| ns != "data");
                }
                eprintln!(
                    "The following namespaces will be deleted:\n  {}",
                    to_delete.join("\n  ")
                );
                eprint!("\nProceed? [y/N] ");
                let mut answer = String::new();
                std::io::stdin().read_line(&mut answer)?;
                if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
                    println!("Aborted.");
                    return Ok(());
                }
            }

            info!(logger, "Tearing down cluster (workflow engine)...");

            let ctx_name = {
                let cfg = crate::config::load_config();
                if cfg.current_context.is_empty() {
                    "default".to_string()
                } else {
                    cfg.current_context.clone()
                }
            };

            let host = crate::workflows::host::create_host(&ctx_name).await?;
            crate::workflows::down::register(&host).await;

            let step_ctx = crate::workflows::StepContext::from_active();
            let effective_profile = profile.or_else(|| {
                if use_lima {
                    Some("lima".to_string())
                } else {
                    None
                }
            });

            let initial_data = serde_json::json!({
                "__ctx": step_ctx,
                "infra": infra,
                "keep_data": keep_data,
                "profile": effective_profile,
                "namespaces_to_delete": [],
                "remaining_namespaces": [],
            });

            let instance = wfe::run_workflow_sync(
                &host,
                "down",
                1,
                initial_data,
                std::time::Duration::from_secs(1800),
            )
            .await
            .map_err(|e| SunbeamError::Other(format!("down workflow failed: {e}")))?;

            crate::workflows::down::print_summary(&logger, &instance);
            crate::workflows::host::shutdown_host(host).await;

            if instance.status != wfe_core::models::WorkflowStatus::Complete {
                return Err(SunbeamError::Other(format!(
                    "down workflow ended with status {:?}",
                    instance.status
                )));
            }

            Ok(())
        }

        Some(Verb::Up {
            set,
            disable,
            enable,
            skip_cilium,
            graph,
            use_lima,
            profile,
            serial,
        }) => {
            if graph {
                let def = crate::workflows::up::definition::build();
                println!("{}", def.to_dot());
                return Ok(());
            }

            let mut overrides =
                crate::manifest_params::Overrides::from_cli(&set, &disable, &enable)?;

            let config = crate::config::load_config();

            // --use-lima implies --profile lima
            let effective_profile = profile.or_else(|| {
                if use_lima {
                    Some("lima".to_string())
                } else {
                    None
                }
            });

            let mut skip_namespaces: Vec<String> = Vec::new();
            let mut skip_ory = false;
            let mut serial_mode = serial;

            // Load profile: CLI --profile first, then active context
            let profile_to_load = effective_profile.clone().or_else(|| {
                let active_ctx = config.contexts.get(&config.current_context)?;
                match &active_ctx.profile {
                    crate::config::ProfileRef::Name(name) => Some(name.clone()),
                    _ => None,
                }
            });

            if let Some(profile_name) = profile_to_load {
                let profile_path = crate::config::get_infra_dir()
                    .join("profiles")
                    .join(format!("{profile_name}.yaml"));

                let profile_obj = if profile_path.exists() {
                    crate::profiles::load_profile(&profile_path)?
                } else if let Some(p) =
                    config.resolve_profile(&crate::config::ProfileRef::Name(profile_name.clone()))
                {
                    p
                } else {
                    return Err(SunbeamError::Config(format!(
                        "Profile not found: {} (looked at {} and config.json)",
                        profile_name,
                        profile_path.display()
                    )));
                };

                // Extract workflow flags
                skip_namespaces = profile_obj.skip_namespaces.clone();
                skip_ory = profile_obj.skip_ory;
                serial_mode = profile_obj.serial_mode || serial;

                // Resolve manifest overrides via tunables/shortcuts
                let base_dir = crate::config::get_infra_dir().join("base");
                match crate::profiles::discover_manifests(&base_dir).await {
                    Ok(resources) => {
                        match crate::profiles::resolve_profile_overrides(
                            &profile_obj,
                            &config.presets,
                            &resources,
                        ) {
                            Ok(profile_overrides) => {
                                overrides.items.extend(profile_overrides.items);
                            }
                            Err(e) => {
                                info!(
                                    logger,
                                    "Failed to resolve profile overrides",
                                    error = e.to_string()
                                );
                            }
                        }
                    }
                    Err(e) => {
                        info!(
                            logger,
                            "Failed to discover manifests for profile resolution",
                            error = e.to_string()
                        );
                    }
                }
            }

            info!(logger, "Bringing up cluster (workflow engine)...");

            let ctx_name = if config.current_context.is_empty() {
                "default".to_string()
            } else {
                config.current_context.clone()
            };

            let host = crate::workflows::host::create_host(&ctx_name).await?;
            crate::workflows::up::register(&host).await;

            let step_ctx = crate::workflows::StepContext::from_active();
            let mut initial_data = serde_json::json!({
                "__ctx": step_ctx,
                "skip_cilium": skip_cilium,
                "serial_mode": serial_mode,
                "skip_ory": skip_ory,
                "skip_namespaces": skip_namespaces,
                "profile": effective_profile,
            });
            if !overrides.items.is_empty() {
                initial_data["manifest_overrides"] =
                    serde_json::to_value(&overrides).map_err(|e| {
                        SunbeamError::Other(format!("Failed to serialize overrides: {e}"))
                    })?;
            }

            let instance = wfe::run_workflow_sync(
                &host,
                "up",
                3,
                initial_data,
                std::time::Duration::from_secs(3600),
            )
            .await
            .map_err(|e| SunbeamError::Other(format!("up workflow failed: {e}")))?;

            crate::workflows::up::print_summary(&logger, &instance);
            crate::workflows::host::shutdown_host(host).await;

            if instance.status != wfe_core::models::WorkflowStatus::Complete {
                return Err(SunbeamError::Other(format!(
                    "up workflow ended with status {:?}",
                    instance.status
                )));
            }

            Ok(())
        }

        Some(Verb::Service { action }) => crate::service_cmds::dispatch(logger, action).await,

        Some(Verb::Config { action }) => match action {
            None => {
                use clap::CommandFactory;
                // Print config subcommand help
                let mut cmd = Cli::command();
                let sub = cmd
                    .find_subcommand_mut("config")
                    .expect("config subcommand");
                sub.print_help()?;
                println!();
                Ok(())
            }
            Some(ConfigAction::Set {
                domain: set_domain,
                infra_dir,
                acme_email,
                kube_context,
                context_name,
            }) => {
                let mut config = crate::config::load_config();
                // Determine which context to modify
                let ctx_name = if context_name.is_empty() {
                    if !config.current_context.is_empty() {
                        config.current_context.clone()
                    } else {
                        "production".to_string()
                    }
                } else {
                    context_name
                };

                let ctx = config.contexts.entry(ctx_name.clone()).or_default();
                if !set_domain.is_empty() {
                    ctx.domain = set_domain;
                }
                if !infra_dir.is_empty() {
                    ctx.infra_dir = infra_dir;
                }
                if !acme_email.is_empty() {
                    ctx.acme_email = acme_email;
                }
                if !kube_context.is_empty() {
                    ctx.kube_context = kube_context;
                }
                if config.current_context.is_empty() {
                    config.current_context = ctx_name;
                }
                crate::config::save_config(&config)
            }
            Some(ConfigAction::UseContext { name }) => {
                let mut config = crate::config::load_config();
                if !config.contexts.contains_key(&name) {
                    info!(
                        logger,
                        "Context does not exist, creating empty context",
                        name = name
                    );
                    config
                        .contexts
                        .insert(name.clone(), crate::config::Context::default());
                }
                config.current_context = name.clone();
                crate::config::save_config(&config)?;
                info!(logger, "Switched to context", name = name);
                Ok(())
            }
            Some(ConfigAction::Get) => {
                let config = crate::config::load_config();
                let current = if config.current_context.is_empty() {
                    "(none)"
                } else {
                    &config.current_context
                };
                info!(logger, "Current context", context = current);
                println!();
                for (name, ctx) in &config.contexts {
                    let marker = if name == current { " *" } else { "" };
                    info!(logger, "Context", name = name, marker = marker);
                    if !ctx.domain.is_empty() {
                        info!(logger, "domain", domain = ctx.domain);
                    }
                    if !ctx.kube_context.is_empty() {
                        info!(logger, "kube-context", kube_context = ctx.kube_context);
                    }
                    if !ctx.infra_dir.is_empty() {
                        info!(logger, "infra-dir", infra_dir = ctx.infra_dir);
                    }
                    if !ctx.acme_email.is_empty() {
                        info!(logger, "acme-email", acme_email = ctx.acme_email);
                    }
                    println!();
                }
                Ok(())
            }
            Some(ConfigAction::Clear) => crate::config::clear_config(),
        },

        Some(Verb::Secrets {
            addr,
            token,
            output,
            action,
        }) => crate::secrets_cli::dispatch(addr.as_deref(), token.as_deref(), output, action).await,

        Some(Verb::User { action }) => match action {
            None => {
                use clap::CommandFactory;
                let mut cmd = Cli::command();
                let sub = cmd.find_subcommand_mut("user").expect("user subcommand");
                sub.print_help()?;
                println!();
                Ok(())
            }
            Some(UserAction::List { search }) => crate::users::cmd_user_list(&search).await,
            Some(UserAction::Get { target }) => crate::users::cmd_user_get(&target).await,
            Some(UserAction::Create {
                email,
                name,
                schema,
            }) => crate::users::cmd_user_create(&email, &name, &schema).await,
            Some(UserAction::Delete { target }) => crate::users::cmd_user_delete(&target).await,
            Some(UserAction::Recover { target }) => crate::users::cmd_user_recover(&target).await,
            Some(UserAction::Disable { target }) => crate::users::cmd_user_disable(&target).await,
            Some(UserAction::Enable { target }) => crate::users::cmd_user_enable(&target).await,
            Some(UserAction::SetPassword { target, password }) => {
                let pw = match password {
                    Some(p) => p,
                    None => {
                        eprint!("Password: ");
                        let mut pw = String::new();
                        std::io::stdin().read_line(&mut pw)?;
                        pw.trim().to_string()
                    }
                };
                crate::users::cmd_user_set_password(&target, &pw).await
            }
            Some(UserAction::Onboard {
                email,
                name,
                schema,
                no_email,
                notify,
                job_title,
                department,
                office_location,
                hire_date,
                manager,
            }) => {
                crate::users::cmd_user_onboard(
                    &email,
                    &name,
                    &schema,
                    !no_email,
                    &notify,
                    &job_title,
                    &department,
                    &office_location,
                    &hire_date,
                    &manager,
                )
                .await
            }
            Some(UserAction::Offboard { target }) => crate::users::cmd_user_offboard(&target).await,
        },

        Some(Verb::Auth { action }) => match action {
            None => crate::auth::cmd_auth_status().await,
            Some(AuthAction::Login { domain, device: true }) => {
                crate::auth::cmd_auth_device_login(domain.as_deref()).await?;
                crate::auth::cmd_auth_git_login(domain.as_deref()).await
            }
            Some(AuthAction::Login { domain, device: false }) => {
                crate::auth::cmd_auth_login_all(domain.as_deref()).await
            }
            Some(AuthAction::Sso { domain, device: true }) => {
                crate::auth::cmd_auth_device_login(domain.as_deref()).await
            }
            Some(AuthAction::Sso { domain, device: false }) => {
                crate::auth::cmd_auth_sso_login(domain.as_deref()).await
            }
            Some(AuthAction::Device { domain }) => {
                crate::auth::cmd_auth_device_login(domain.as_deref()).await
            }
            Some(AuthAction::Git { domain }) => {
                crate::auth::cmd_auth_git_login(domain.as_deref()).await
            }
            Some(AuthAction::Logout) => crate::auth::cmd_auth_logout().await,
            Some(AuthAction::Status) => crate::auth::cmd_auth_status().await,
            Some(AuthAction::Token) => crate::auth::cmd_auth_token().await,
        },

        Some(Verb::Workflow {
            target,
            output,
            action,
        }) => crate::workflows::cmd::dispatch(Some(&target), action, output).await,

        Some(Verb::Kanban {
            output,
            url,
            action,
        }) => crate::kanban::dispatch(logger, action, output, url.as_deref()).await,

        Some(Verb::Vpn { action }) => match action {
            VpnAction::Status => crate::vpn_cmds::cmd_vpn_status().await,
            VpnAction::Connect { foreground } => crate::vpn_cmds::cmd_connect(foreground).await,
            VpnAction::Disconnect => crate::vpn_cmds::cmd_disconnect().await,
            VpnAction::CreateKey {
                user,
                user_id,
                reusable,
                ephemeral,
                expiration,
            } => {
                crate::vpn_cmds::cmd_vpn_create_key(
                    &user,
                    user_id,
                    reusable,
                    ephemeral,
                    &expiration,
                )
                .await
            }
        },

        Some(Verb::Completions { shell }) => {
            use clap::CommandFactory;
            clap_complete::generate(
                shell,
                &mut Cli::command(),
                "sunbeam",
                &mut std::io::stdout(),
            );
            Ok(())
        }

        Some(Verb::Doctor) => crate::doctor::cmd_doctor(&logger).await,

        Some(Verb::VpnDaemon) => crate::vpn_cmds::cmd_vpn_daemon().await,

        Some(Verb::Update) => crate::update::cmd_update(&logger).await,

        Some(Verb::Version) => {
            crate::update::cmd_version();
            Ok(())
        }

        Some(Verb::Project { action }) => crate::project::cli::dispatch(logger, action).await,

        Some(Verb::Operations { action }) => crate::operations::cli::dispatch(logger, action).await,

        Some(Verb::Vcs { action }) => {
            let logger = crate::logger::Logger::new(crate::logger::TracingSink);
            crate::vcs::dispatch(&logger, action).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).unwrap()
    }

    // 1. test_up
    #[test]
    fn test_up() {
        let cli = parse(&["sunbeam", "up"]);
        assert!(matches!(cli.verb, Some(Verb::Up { .. })));
    }

    #[test]
    fn test_up_graph_flag() {
        let cli = parse(&["sunbeam", "up", "--graph"]);
        match cli.verb {
            Some(Verb::Up { graph, .. }) => assert!(graph),
            _ => panic!("expected Up with --graph"),
        }
    }

    #[test]
    fn test_up_profile_flag() {
        let cli = parse(&["sunbeam", "up", "--profile", "lima"]);
        match cli.verb {
            Some(Verb::Up { profile, .. }) => assert_eq!(profile, Some("lima".to_string())),
            _ => panic!("expected Up with --profile"),
        }
    }

    #[test]
    fn test_up_use_lima_flag() {
        let cli = parse(&["sunbeam", "up", "--use-lima"]);
        match cli.verb {
            Some(Verb::Up { use_lima, .. }) => assert!(use_lima),
            _ => panic!("expected Up with --use-lima"),
        }
    }

    #[test]
    fn test_up_serial_flag() {
        let cli = parse(&["sunbeam", "up", "--serial"]);
        match cli.verb {
            Some(Verb::Up { serial, .. }) => assert!(serial),
            _ => panic!("expected Up with --serial"),
        }
    }

    #[test]
    fn test_down_profile_flag() {
        let cli = parse(&["sunbeam", "down", "--profile", "lima"]);
        match cli.verb {
            Some(Verb::Down { profile, .. }) => assert_eq!(profile, Some("lima".to_string())),
            _ => panic!("expected Down with --profile"),
        }
    }

    #[test]
    fn test_service_status_no_target() {
        let cli = parse(&["sunbeam", "service", "status"]);
        match cli.verb {
            Some(Verb::Service {
                action: ServiceAction::Status { target },
            }) => assert!(target.is_none()),
            _ => panic!("expected Service Status"),
        }
    }

    #[test]
    fn test_service_status_with_namespace() {
        let cli = parse(&["sunbeam", "service", "status", "ory"]);
        match cli.verb {
            Some(Verb::Service {
                action: ServiceAction::Status { target },
            }) => assert_eq!(target.unwrap(), "ory"),
            _ => panic!("expected Service Status"),
        }
    }

    #[test]
    fn test_service_logs_no_follow() {
        let cli = parse(&["sunbeam", "service", "logs", "ory/kratos"]);
        match cli.verb {
            Some(Verb::Service {
                action: ServiceAction::Logs { target, follow },
            }) => {
                assert_eq!(target, "ory/kratos");
                assert!(!follow);
            }
            _ => panic!("expected Service Logs"),
        }
    }

    #[test]
    fn test_service_logs_follow_short() {
        let cli = parse(&["sunbeam", "service", "logs", "ory/kratos", "-f"]);
        match cli.verb {
            Some(Verb::Service {
                action: ServiceAction::Logs { follow, .. },
            }) => assert!(follow),
            _ => panic!("expected Service Logs"),
        }
    }

    // 9. test_user_set_password
    #[test]
    fn test_user_set_password() {
        let cli = parse(&[
            "sunbeam",
            "user",
            "set-password",
            "admin@example.com",
            "hunter2",
        ]);
        match cli.verb {
            Some(Verb::User {
                action: Some(UserAction::SetPassword { target, password }),
            }) => {
                assert_eq!(target, "admin@example.com");
                assert_eq!(password, Some("hunter2".to_string()));
            }
            _ => panic!("expected User SetPassword"),
        }
    }

    #[test]
    fn test_user_set_password_no_password() {
        let cli = parse(&["sunbeam", "user", "set-password", "admin@example.com"]);
        match cli.verb {
            Some(Verb::User {
                action: Some(UserAction::SetPassword { target, password }),
            }) => {
                assert_eq!(target, "admin@example.com");
                assert!(password.is_none());
            }
            _ => panic!("expected User SetPassword"),
        }
    }

    // 10. test_user_onboard_basic
    #[test]
    fn test_user_onboard_basic() {
        let cli = parse(&["sunbeam", "user", "onboard", "a@b.com"]);
        match cli.verb {
            Some(Verb::User {
                action:
                    Some(UserAction::Onboard {
                        email,
                        name,
                        schema,
                        no_email,
                        notify,
                        ..
                    }),
            }) => {
                assert_eq!(email, "a@b.com");
                assert_eq!(name, "");
                assert_eq!(schema, "employee");
                assert!(!no_email);
                assert_eq!(notify, "");
            }
            _ => panic!("expected User Onboard"),
        }
    }

    // 11. test_user_onboard_full
    #[test]
    fn test_user_onboard_full() {
        let cli = parse(&[
            "sunbeam",
            "user",
            "onboard",
            "a@b.com",
            "--name",
            "A B",
            "--schema",
            "default",
            "--no-email",
            "--job-title",
            "Engineer",
            "--department",
            "Dev",
            "--office-location",
            "Paris",
            "--hire-date",
            "2026-01-15",
            "--manager",
            "boss@b.com",
        ]);
        match cli.verb {
            Some(Verb::User {
                action:
                    Some(UserAction::Onboard {
                        email,
                        name,
                        schema,
                        no_email,
                        job_title,
                        department,
                        office_location,
                        hire_date,
                        manager,
                        ..
                    }),
            }) => {
                assert_eq!(email, "a@b.com");
                assert_eq!(name, "A B");
                assert_eq!(schema, "default");
                assert!(no_email);
                assert_eq!(job_title, "Engineer");
                assert_eq!(department, "Dev");
                assert_eq!(office_location, "Paris");
                assert_eq!(hire_date, "2026-01-15");
                assert_eq!(manager, "boss@b.com");
            }
            _ => panic!("expected User Onboard"),
        }
    }

    #[test]
    fn test_service_apply_no_namespace() {
        let cli = parse(&["sunbeam", "service", "apply"]);
        match cli.verb {
            Some(Verb::Service {
                action:
                    ServiceAction::Apply {
                        namespace, dry_run, ..
                    },
            }) => {
                assert!(namespace.is_none());
                assert!(!dry_run);
            }
            _ => panic!("expected Service Apply"),
        }
    }

    #[test]
    fn test_service_apply_with_namespace() {
        let cli = parse(&["sunbeam", "service", "apply", "ory"]);
        match cli.verb {
            Some(Verb::Service {
                action: ServiceAction::Apply { namespace, .. },
            }) => assert_eq!(namespace.unwrap(), "ory"),
            _ => panic!("expected Service Apply"),
        }
    }

    #[test]
    fn test_service_apply_dry_run() {
        let cli = parse(&["sunbeam", "service", "apply", "--dry-run"]);
        match cli.verb {
            Some(Verb::Service {
                action: ServiceAction::Apply { dry_run, .. },
            }) => assert!(dry_run),
            _ => panic!("expected Service Apply --dry-run"),
        }
    }

    #[test]
    fn test_service_apply_dry_run_with_namespace() {
        let cli = parse(&["sunbeam", "service", "apply", "ory", "--dry-run"]);
        match cli.verb {
            Some(Verb::Service {
                action:
                    ServiceAction::Apply {
                        namespace, dry_run, ..
                    },
            }) => {
                assert_eq!(namespace.unwrap(), "ory");
                assert!(dry_run);
            }
            _ => panic!("expected Service Apply ory --dry-run"),
        }
    }

    #[test]
    fn test_service_apply_dry_run_with_all() {
        let cli = parse(&["sunbeam", "service", "apply", "--all", "--dry-run"]);
        match cli.verb {
            Some(Verb::Service {
                action:
                    ServiceAction::Apply {
                        apply_all, dry_run, ..
                    },
            }) => {
                assert!(apply_all);
                assert!(dry_run);
            }
            _ => panic!("expected Service Apply --all --dry-run"),
        }
    }

    // 14. test_config_set
    #[test]
    fn test_config_set() {
        let cli = parse(&[
            "sunbeam",
            "config",
            "set",
            "--domain",
            "example.com",
            "--infra-dir",
            "/path/to/infra",
        ]);
        match cli.verb {
            Some(Verb::Config {
                action:
                    Some(ConfigAction::Set {
                        domain, infra_dir, ..
                    }),
            }) => {
                assert_eq!(domain, "example.com");
                assert_eq!(infra_dir, "/path/to/infra");
            }
            _ => panic!("expected Config Set"),
        }
    }

    // 15. test_config_get / test_config_clear
    #[test]
    fn test_config_get() {
        let cli = parse(&["sunbeam", "config", "get"]);
        match cli.verb {
            Some(Verb::Config {
                action: Some(ConfigAction::Get),
            }) => {}
            _ => panic!("expected Config Get"),
        }
    }

    #[test]
    fn test_config_clear() {
        let cli = parse(&["sunbeam", "config", "clear"]);
        match cli.verb {
            Some(Verb::Config {
                action: Some(ConfigAction::Clear),
            }) => {}
            _ => panic!("expected Config Clear"),
        }
    }

    // 16. test_no_args_prints_help
    #[test]
    fn test_no_args_prints_help() {
        let cli = parse(&["sunbeam"]);
        assert!(cli.verb.is_none());
    }

    #[test]
    fn test_service_get_json_output() {
        let cli = parse(&["sunbeam", "service", "get", "ory/kratos-abc", "-o", "json"]);
        match cli.verb {
            Some(Verb::Service {
                action: ServiceAction::Get { target, output },
            }) => {
                assert_eq!(target, "ory/kratos-abc");
                assert_eq!(output, "json");
            }
            _ => panic!("expected Service Get"),
        }
    }

    #[test]
    fn test_service_check_with_target() {
        let cli = parse(&["sunbeam", "service", "check", "devtools"]);
        match cli.verb {
            Some(Verb::Service {
                action: ServiceAction::Check { target },
            }) => assert_eq!(target.unwrap(), "devtools"),
            _ => panic!("expected Service Check"),
        }
    }

    // 20. test_hire_date_validation
    #[test]
    fn test_hire_date_valid() {
        let cli = parse(&[
            "sunbeam",
            "user",
            "onboard",
            "a@b.com",
            "--hire-date",
            "2026-01-15",
        ]);
        match cli.verb {
            Some(Verb::User {
                action: Some(UserAction::Onboard { hire_date, .. }),
            }) => {
                assert_eq!(hire_date, "2026-01-15");
            }
            _ => panic!("expected User Onboard"),
        }
    }

    #[test]
    fn test_hire_date_invalid() {
        let result = Cli::try_parse_from([
            "sunbeam",
            "user",
            "onboard",
            "a@b.com",
            "--hire-date",
            "not-a-date",
        ]);
        assert!(result.is_err());
    }

    // -- Workflow subcommand tests --

    #[test]
    fn test_workflow_list() {
        let cli = parse(&["sunbeam", "workflow", "list"]);
        match cli.verb {
            Some(Verb::Workflow { action, .. }) => {
                assert!(matches!(
                    action,
                    crate::workflows::cmd::WorkflowAction::List { .. }
                ));
            }
            _ => panic!("expected Workflow List"),
        }
    }

    #[test]
    fn test_workflow_list_with_status_filter() {
        let cli = parse(&["sunbeam", "workflow", "list", "--status", "complete"]);
        match cli.verb {
            Some(Verb::Workflow { action, .. }) => match action {
                crate::workflows::cmd::WorkflowAction::List { status, .. } => {
                    assert_eq!(status, "complete");
                }
                _ => panic!("expected List"),
            },
            _ => panic!("expected Workflow"),
        }
    }

    #[test]
    fn test_workflow_status() {
        let cli = parse(&["sunbeam", "workflow", "status", "abc-123"]);
        match cli.verb {
            Some(Verb::Workflow { action, .. }) => match action {
                crate::workflows::cmd::WorkflowAction::Status { id } => {
                    assert_eq!(id, "abc-123");
                }
                _ => panic!("expected Status"),
            },
            _ => panic!("expected Workflow"),
        }
    }

    #[test]
    fn test_workflow_retry() {
        let cli = parse(&["sunbeam", "workflow", "retry", "wf-456"]);
        match cli.verb {
            Some(Verb::Workflow { action, .. }) => match action {
                crate::workflows::cmd::WorkflowAction::Retry { id } => {
                    assert_eq!(id, "wf-456");
                }
                _ => panic!("expected Retry"),
            },
            _ => panic!("expected Workflow"),
        }
    }

    #[test]
    fn test_workflow_cancel() {
        let cli = parse(&["sunbeam", "workflow", "cancel", "wf-789"]);
        match cli.verb {
            Some(Verb::Workflow { action, .. }) => match action {
                crate::workflows::cmd::WorkflowAction::Cancel { id } => {
                    assert_eq!(id, "wf-789");
                }
                _ => panic!("expected Cancel"),
            },
            _ => panic!("expected Workflow"),
        }
    }

    #[test]
    fn test_workflow_run_default_file() {
        let cli = parse(&["sunbeam", "workflow", "run"]);
        match cli.verb {
            Some(Verb::Workflow { action, .. }) => match action {
                crate::workflows::cmd::WorkflowAction::Run { file } => {
                    assert_eq!(file, "");
                }
                _ => panic!("expected Run"),
            },
            _ => panic!("expected Workflow"),
        }
    }

    #[test]
    fn test_workflow_run_with_file() {
        let cli = parse(&["sunbeam", "workflow", "run", "deploy.yaml"]);
        match cli.verb {
            Some(Verb::Workflow { action, .. }) => match action {
                crate::workflows::cmd::WorkflowAction::Run { file } => {
                    assert_eq!(file, "deploy.yaml");
                }
                _ => panic!("expected Run"),
            },
            _ => panic!("expected Workflow"),
        }
    }

    #[test]
    fn test_workflow_status_missing_id() {
        let result = Cli::try_parse_from(["sunbeam", "workflow", "status"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_service_deploy_no_target() {
        let cli = parse(&["sunbeam", "service", "deploy"]);
        match cli.verb {
            Some(Verb::Service {
                action:
                    ServiceAction::Deploy {
                        target,
                        all,
                        profile,
                    },
            }) => {
                assert!(target.is_none());
                assert!(!all);
                assert!(profile.is_none());
            }
            _ => panic!("expected Service Deploy"),
        }
    }

    #[test]
    fn test_service_deploy_with_target() {
        let cli = parse(&["sunbeam", "service", "deploy", "hydra"]);
        match cli.verb {
            Some(Verb::Service {
                action: ServiceAction::Deploy { target, .. },
            }) => {
                assert_eq!(target.unwrap(), "hydra");
            }
            _ => panic!("expected Service Deploy"),
        }
    }

    #[test]
    fn test_service_deploy_all() {
        let cli = parse(&["sunbeam", "service", "deploy", "--all"]);
        match cli.verb {
            Some(Verb::Service {
                action: ServiceAction::Deploy { all, .. },
            }) => assert!(all),
            _ => panic!("expected Service Deploy"),
        }
    }

    #[test]
    fn test_service_secrets_list() {
        let cli = parse(&["sunbeam", "service", "secrets", "hydra"]);
        match cli.verb {
            Some(Verb::Service {
                action: ServiceAction::Secrets { service, action },
            }) => {
                assert_eq!(service, "hydra");
                assert!(action.is_none());
            }
            _ => panic!("expected Service Secrets"),
        }
    }

    #[test]
    fn test_service_secrets_get() {
        let cli = parse(&[
            "sunbeam",
            "service",
            "secrets",
            "hydra",
            "get",
            "system-secret",
        ]);
        match cli.verb {
            Some(Verb::Service {
                action: ServiceAction::Secrets { service, action },
            }) => {
                assert_eq!(service, "hydra");
                match action {
                    Some(SecretsAction::Get { key }) => assert_eq!(key, "system-secret"),
                    _ => panic!("expected Get"),
                }
            }
            _ => panic!("expected Service Secrets"),
        }
    }

    #[test]
    fn test_service_shell() {
        let cli = parse(&["sunbeam", "service", "shell", "postgres"]);
        match cli.verb {
            Some(Verb::Service {
                action: ServiceAction::Shell { service },
            }) => {
                assert_eq!(service, "postgres");
            }
            _ => panic!("expected Service Shell"),
        }
    }

    #[test]
    fn test_service_describe() {
        let cli = parse(&["sunbeam", "service", "describe", "hydra"]);
        match cli.verb {
            Some(Verb::Service {
                action: ServiceAction::Describe { service },
            }) => {
                assert_eq!(service, "hydra");
            }
            _ => panic!("expected Service Describe"),
        }
    }

    #[test]
    fn test_service_scale() {
        let cli = parse(&["sunbeam", "service", "scale", "hydra", "3"]);
        match cli.verb {
            Some(Verb::Service {
                action: ServiceAction::Scale { service, replicas },
            }) => {
                assert_eq!(service, "hydra");
                assert_eq!(replicas, 3);
            }
            _ => panic!("expected Service Scale"),
        }
    }

    #[test]
    fn test_service_port_forward() {
        let cli = parse(&["sunbeam", "service", "port-forward", "hydra", "8080:80"]);
        match cli.verb {
            Some(Verb::Service {
                action: ServiceAction::PortForward { service, ports },
            }) => {
                assert_eq!(service, "hydra");
                assert_eq!(ports, vec!["8080:80"]);
            }
            _ => panic!("expected Service PortForward"),
        }
    }

    #[test]
    fn test_svc_alias() {
        let cli = parse(&["sunbeam", "svc", "top", "hydra"]);
        match cli.verb {
            Some(Verb::Service {
                action: ServiceAction::Top { service },
            }) => {
                assert_eq!(service, "hydra");
            }
            _ => panic!("expected Service Top via svc alias"),
        }
    }

    #[test]
    fn test_project_build_parses() {
        let cli = parse(&["sunbeam", "project", "build"]);
        match cli.verb {
            Some(Verb::Project {
                action: ProjectAction::Build(args),
            }) => {
                assert!(!args.all);
                assert!(args.projects.is_empty());
            }
            _ => panic!("expected Project Build"),
        }
    }

    #[test]
    fn test_project_build_all() {
        let cli = parse(&["sunbeam", "project", "build", "--all"]);
        match cli.verb {
            Some(Verb::Project {
                action: ProjectAction::Build(args),
            }) => {
                assert!(args.all);
            }
            _ => panic!("expected Project Build --all"),
        }
    }

    #[test]
    fn test_project_build_with_p_flag() {
        let cli = parse(&["sunbeam", "project", "build", "-p", "cli", "-p", "sol"]);
        match cli.verb {
            Some(Verb::Project {
                action: ProjectAction::Build(args),
            }) => {
                assert_eq!(args.projects, vec!["cli", "sol"]);
            }
            _ => panic!("expected Project Build -p"),
        }
    }

    #[test]
    fn test_ops_compose_up_parses() {
        let cli = parse(&["sunbeam", "operations", "compose", "up"]);
        match cli.verb {
            Some(Verb::Operations {
                action:
                    OperationsAction::Compose {
                        action: ComposeAction::Up { services, wait },
                    },
            }) => {
                assert!(services.is_empty());
                assert!(wait);
            }
            _ => panic!("expected Operations Compose Up"),
        }
    }

    #[test]
    fn test_ops_compose_down_volumes() {
        let cli = parse(&["sunbeam", "ops", "compose", "down", "--volumes"]);
        match cli.verb {
            Some(Verb::Operations {
                action:
                    OperationsAction::Compose {
                        action: ComposeAction::Down { volumes },
                    },
            }) => {
                assert!(volumes);
            }
            _ => panic!("expected Operations Compose Down --volumes"),
        }
    }

    #[test]
    fn test_ops_stack_pin() {
        let cli = parse(&[
            "sunbeam",
            "ops",
            "stack",
            "pin",
            "release-1.0",
            "cli",
            "sol",
            "--description",
            "initial release",
        ]);
        match cli.verb {
            Some(Verb::Operations {
                action:
                    OperationsAction::Stack {
                        action:
                            StackAction::Pin {
                                name,
                                projects,
                                description,
                            },
                    },
            }) => {
                assert_eq!(name, "release-1.0");
                assert_eq!(projects, vec!["cli", "sol"]);
                assert_eq!(description.as_deref(), Some("initial release"));
            }
            _ => panic!("expected Operations Stack Pin"),
        }
    }

    #[test]
    fn test_project_run_custom_verb() {
        let cli = parse(&["sunbeam", "project", "run", "seed"]);
        match cli.verb {
            Some(Verb::Project {
                action: ProjectAction::Run { verb, args },
            }) => {
                assert_eq!(verb, "seed");
                assert!(!args.all);
            }
            _ => panic!("expected Project Run"),
        }
    }

    #[test]
    fn test_project_graph_all() {
        let cli = parse(&["sunbeam", "project", "graph", "--all"]);
        match cli.verb {
            Some(Verb::Project {
                action: ProjectAction::Graph { all },
            }) => {
                assert!(all);
            }
            _ => panic!("expected Project Graph"),
        }
    }

    #[test]
    fn test_project_check() {
        let cli = parse(&["sunbeam", "project", "check"]);
        match cli.verb {
            Some(Verb::Project {
                action: ProjectAction::Check { all },
            }) => {
                assert!(!all);
            }
            _ => panic!("expected Project Check"),
        }
    }

    // -- Secrets subcommand tests --

    #[test]
    fn test_secrets_kv_get_parses() {
        let cli = parse(&["sunbeam", "secrets", "kv", "get", "secret/hydra"]);
        match cli.verb {
            Some(Verb::Secrets { action, .. }) => {
                assert!(matches!(
                    action,
                    crate::secrets_cli::SecretsAction::Kv(crate::secrets_cli::KvAction::Get { .. })
                ));
            }
            _ => panic!("expected Secrets Kv Get"),
        }
    }

    #[test]
    fn test_secrets_kv_put_parses() {
        let cli = parse(&["sunbeam", "secrets", "kv", "put", "secret/hydra", "foo=bar"]);
        match cli.verb {
            Some(Verb::Secrets { action, .. }) => match action {
                crate::secrets_cli::SecretsAction::Kv(crate::secrets_cli::KvAction::Put {
                    path,
                    pairs,
                    ..
                }) => {
                    assert_eq!(path, "secret/hydra");
                    assert_eq!(pairs, vec!["foo=bar"]);
                }
                _ => panic!("expected Kv Put"),
            },
            _ => panic!("expected Secrets"),
        }
    }

    #[test]
    fn test_secrets_transit_enable_parses() {
        let cli = parse(&["sunbeam", "secrets", "transit", "enable", "transit/sbbb"]);
        match cli.verb {
            Some(Verb::Secrets { action, .. }) => match action {
                crate::secrets_cli::SecretsAction::Transit(
                    crate::secrets_cli::TransitAction::Enable { mount },
                ) => {
                    assert_eq!(mount, "transit/sbbb");
                }
                _ => panic!("expected Transit Enable"),
            },
            _ => panic!("expected Secrets"),
        }
    }

    #[test]
    fn test_secrets_status_parses() {
        let cli = parse(&["sunbeam", "secrets", "status"]);
        match cli.verb {
            Some(Verb::Secrets { action, .. }) => {
                assert!(matches!(action, crate::secrets_cli::SecretsAction::Status));
            }
            _ => panic!("expected Secrets Status"),
        }
    }

    #[test]
    fn test_secrets_exec_parses() {
        let cli = parse(&["sunbeam", "secrets", "exec", "kv", "list", "secret/"]);
        match cli.verb {
            Some(Verb::Secrets { action, .. }) => match action {
                crate::secrets_cli::SecretsAction::Exec { args } => {
                    assert_eq!(args, vec!["kv", "list", "secret/"]);
                }
                _ => panic!("expected Exec"),
            },
            _ => panic!("expected Secrets"),
        }
    }

    #[test]
    fn test_secrets_with_addr_and_token() {
        let cli = parse(&[
            "sunbeam",
            "secrets",
            "--addr",
            "https://vault.example.com:8200",
            "--token",
            "hvs.test",
            "status",
        ]);
        match cli.verb {
            Some(Verb::Secrets {
                addr,
                token,
                action,
                ..
            }) => {
                assert_eq!(addr, Some("https://vault.example.com:8200".to_string()));
                assert_eq!(token, Some("hvs.test".to_string()));
                assert!(matches!(action, crate::secrets_cli::SecretsAction::Status));
            }
            _ => panic!("expected Secrets with addr+token"),
        }
    }
}
