use crate::error::{Result, SunbeamError};
use clap::{Parser, Subcommand};
use clap_complete::Shell;

/// Sunbeam local dev stack manager.
#[derive(Parser, Debug)]
#[command(name = "sunbeam", about = "Sunbeam local dev stack manager")]
pub struct Cli {
    /// Named context to use (overrides current-context from config).
    #[arg(long)]
    pub context: Option<String>,

    /// Domain suffix override (e.g. sunbeam.pt).
    #[arg(long, default_value = "")]
    pub domain: String,

    /// ACME email for cert-manager (e.g. ops@sunbeam.pt).
    #[arg(long, default_value = "")]
    pub email: String,

    #[command(subcommand)]
    pub verb: Option<Verb>,
}

#[derive(Subcommand, Debug)]
pub enum Verb {
    /// Full cluster bring-up.
    Up,

    /// Manage sunbeam configuration.
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,
    },

    /// User/identity management.
    User {
        #[command(subcommand)]
        action: Option<UserAction>,
    },

    /// Authenticate with Sunbeam (OAuth2 login via browser).
    Auth {
        #[command(subcommand)]
        action: Option<AuthAction>,
    },

    /// Project management across Planka and Gitea.
    Pm {
        #[command(subcommand)]
        action: Option<PmAction>,
    },

    /// Local workflow management (list, status, retry, cancel, run).
    Workflow {
        #[command(subcommand)]
        action: crate::workflows::cmd::WorkflowAction,
    },

    /// Remote workflow management (via wfe-server).
    Workflows {
        /// Output format.
        #[arg(short, long, value_enum, default_value_t = crate::wfectl::output::OutputFormat::Table, global = true)]
        output: crate::wfectl::output::OutputFormat,
        #[command(subcommand)]
        action: crate::wfectl::WorkflowsCommand,
    },

    /// Service operations (deploy, logs, restart, exec, secrets, ...).
    #[command(alias = "svc")]
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },

    /// VPN management.
    Vpn {
        #[command(subcommand)]
        action: VpnAction,
    },

    /// bao CLI passthrough (runs inside OpenBao pod with root token).
    #[command(hide = true)]
    Bao {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        bao_args: Vec<String>,
    },

    /// Generate shell completions.
    Completions {
        /// Shell to generate completions for.
        #[arg(value_enum)]
        shell: Shell,
    },

    /// Connectivity diagnostics.
    Doctor,

    /// Internal: run the VPN daemon in the foreground.
    #[command(name = "__vpn-daemon", hide = true)]
    VpnDaemon,

    /// Self-update from latest mainline commit.
    Update,

    /// Print version info.
    Version,

    /// Per-project build verbs (alias: proj).
    #[command(alias = "proj")]
    Project {
        #[command(subcommand)]
        action: ProjectAction,
    },

    /// Workspace-level operations (alias: ops).
    #[command(alias = "ops")]
    Operations {
        #[command(subcommand)]
        action: OperationsAction,
    },

    /// Shortcut for `sunbeam ops worktree` — per-branch git worktree lifecycle.
    Wt {
        #[command(subcommand)]
        action: WorktreeAction,
    },

    /// Build the current project (shortcut for `sunbeam project build`).
    Build {
        #[command(flatten)]
        args: ProjectRunArgs,
    },
    /// Run the current project's tests (shortcut for `sunbeam project test`).
    Test {
        #[command(flatten)]
        args: ProjectRunArgs,
    },
    /// Lint the current project (shortcut for `sunbeam project lint`).
    Lint {
        #[command(flatten)]
        args: ProjectRunArgs,
    },
    /// Format the current project (shortcut for `sunbeam project fmt`).
    Fmt {
        #[command(flatten)]
        args: ProjectRunArgs,
    },
    /// Package the current project (shortcut for `sunbeam project package`).
    Package {
        #[command(flatten)]
        args: ProjectRunArgs,
    },
    /// Deploy the current project (shortcut for `sunbeam project deploy`).
    Deploy {
        #[command(flatten)]
        args: ProjectRunArgs,
    },
    /// Run the current project's dev server (shortcut for `sunbeam project dev`).
    Dev {
        #[command(flatten)]
        args: ProjectRunArgs,
    },
    /// Clean the current project's build artifacts (shortcut for `sunbeam project clean`).
    Clean {
        #[command(flatten)]
        args: ProjectRunArgs,
    },
    /// Generate the current project's docs (shortcut for `sunbeam project doc`).
    Doc {
        #[command(flatten)]
        args: ProjectRunArgs,
    },
}

#[derive(Subcommand, Debug)]
pub enum VpnAction {
    /// Show VPN tunnel status.
    Status,
    /// Connect to the cluster VPN.
    Connect {
        /// Run the daemon in the foreground instead of detaching.
        #[arg(long)]
        foreground: bool,
    },
    /// Disconnect from the cluster VPN.
    Disconnect,
    /// Create a new pre-auth key for onboarding a new client.
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

#[derive(Subcommand, Debug)]
pub enum ServiceAction {
    /// Pod health (optionally scoped).
    Status {
        /// Service, namespace, or namespace/name.
        target: Option<String>,
    },

    /// kubectl logs for a service.
    Logs {
        /// Service or namespace/name.
        target: String,
        /// Stream logs.
        #[arg(short, long)]
        follow: bool,
    },

    /// Raw kubectl get for a pod (ns/name).
    Get {
        /// Service or namespace/name.
        target: String,
        /// Output format.
        #[arg(short, long, default_value = "yaml", value_parser = ["yaml", "json", "wide"])]
        output: String,
    },

    /// Rolling restart of services.
    Restart {
        /// Service, namespace, or namespace/name.
        target: Option<String>,
    },

    /// Functional service health checks.
    Check {
        /// Service, namespace, or namespace/name.
        target: Option<String>,
    },

    /// Deploy service(s) — apply manifests + rollout restart.
    Deploy {
        /// Service name, category, or namespace (e.g. "hydra", "auth", "ory").
        /// Use --all for everything.
        target: Option<String>,
        /// Deploy all services.
        #[arg(long)]
        all: bool,
    },

    /// kustomize build + domain subst + kubectl apply.
    Apply {
        /// Limit apply to one namespace.
        namespace: Option<String>,
        /// Apply all namespaces without confirmation.
        #[arg(long = "all")]
        apply_all: bool,
        /// Domain suffix (e.g. sunbeam.pt).
        #[arg(long, default_value = "")]
        domain: String,
        /// ACME email for cert-manager.
        #[arg(long, default_value = "")]
        email: String,
        /// Print the post-substitution YAML without calling kubectl apply.
        #[arg(long)]
        dry_run: bool,
    },

    /// Generate/store all credentials in OpenBao.
    Seed,

    /// E2E VSO + OpenBao integration test.
    Verify,

    /// View or get secrets for a service from OpenBao.
    Secrets {
        /// Service name (e.g. "hydra").
        service: String,
        #[command(subcommand)]
        action: Option<SecretsAction>,
    },

    /// Interactive shell into a service pod.
    Shell {
        /// Service name (e.g. "postgres", "gitea").
        service: String,
    },

    /// Describe a service (kubectl describe on its deployment).
    Describe {
        /// Service name.
        service: String,
    },

    /// Exec into a service pod.
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
    PortForward {
        /// Service name.
        service: String,
        /// Port mapping (e.g. "8080:80", "8080").
        ports: Vec<String>,
    },

    /// Scale a service deployment.
    Scale {
        /// Service name.
        service: String,
        /// Number of replicas.
        replicas: u32,
    },

    /// Show resource usage (CPU/memory) for a service's pods.
    Top {
        /// Service name.
        service: String,
    },

    /// Edit a service's deployment manifest in-cluster.
    Edit {
        /// Service name.
        service: String,
    },

    /// Manage OpenBao Transit secrets engine (mounts, keys, signing).
    Transit {
        #[command(subcommand)]
        action: TransitAction,
    },

    /// Delete a Job by namespace/name (workaround for k8s Job immutability).
    DeleteJob {
        /// Job reference in the form <namespace>/<name>.
        target: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum SecretsAction {
    /// Get a specific secret field value.
    Get {
        /// Field name within the service's KV path.
        key: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum TransitAction {
    /// Enable a transit secrets engine at a mount path (idempotent).
    Enable {
        /// Mount path (e.g. "transit/sbbb").
        mount: String,
    },
    /// Create an Ed25519 key under a transit mount (idempotent).
    CreateKey {
        /// Mount path (e.g. "transit/sbbb").
        mount: String,
        /// Key name (e.g. "sbbb-shell").
        name: String,
        /// Key type.
        #[arg(long, default_value = "ed25519")]
        key_type: String,
    },
    /// Read the public metadata of a transit key.
    ReadKey {
        /// Mount path (e.g. "transit/sbbb").
        mount: String,
        /// Key name (e.g. "sbbb-shell").
        name: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum AuthAction {
    /// Log in to both SSO and Gitea.
    Login {
        /// Domain to authenticate against (e.g. sunbeam.pt).
        #[arg(long)]
        domain: Option<String>,
    },
    /// Log in to SSO only (Hydra OIDC — for Planka, identity management).
    Sso {
        /// Domain to authenticate against.
        #[arg(long)]
        domain: Option<String>,
    },
    /// Log in to Gitea only (personal access token).
    Git {
        /// Domain to authenticate against.
        #[arg(long)]
        domain: Option<String>,
    },
    /// Log out (remove all cached tokens).
    Logout,
    /// Show current authentication status.
    Status,
    /// Print the current access token (for use in scripts and MCP headers).
    Token,
}

#[derive(Subcommand, Debug)]
pub enum PmAction {
    /// List tickets across Planka and Gitea.
    List {
        /// Filter by source: planka, gitea, or all (default: all).
        #[arg(long, default_value = "all")]
        source: String,
        /// Filter by state: open, closed, all (default: open).
        #[arg(long, default_value = "open")]
        state: String,
    },
    /// Show ticket details.
    Show {
        /// Ticket ID (e.g. p:42 for Planka, g:studio/cli#7 for Gitea).
        id: String,
    },
    /// Create a new ticket.
    Create {
        /// Ticket title.
        title: String,
        /// Ticket body/description.
        #[arg(long, default_value = "")]
        body: String,
        /// Source: planka or gitea.
        #[arg(long, default_value = "gitea")]
        source: String,
        /// Target: board ID for Planka, or org/repo for Gitea.
        #[arg(long, default_value = "")]
        target: String,
    },
    /// Add a comment to a ticket.
    Comment {
        /// Ticket ID.
        id: String,
        /// Comment text.
        text: String,
    },
    /// Close/complete a ticket.
    Close {
        /// Ticket ID.
        id: String,
    },
    /// Assign a user to a ticket.
    Assign {
        /// Ticket ID.
        id: String,
        /// Username or email to assign.
        user: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum ConfigAction {
    /// Set configuration values for the current context.
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
    Get,
    /// Clear configuration.
    Clear,
    /// Switch the active context.
    UseContext {
        /// Context name to switch to.
        name: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum UserAction {
    /// List identities.
    List {
        /// Filter by email.
        #[arg(long, default_value = "")]
        search: String,
    },
    /// Get identity by email or ID.
    Get {
        /// Email or identity ID.
        target: String,
    },
    /// Create identity.
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
    Delete {
        /// Email or identity ID.
        target: String,
    },
    /// Generate recovery link.
    Recover {
        /// Email or identity ID.
        target: String,
    },
    /// Disable identity + revoke sessions (lockout).
    Disable {
        /// Email or identity ID.
        target: String,
    },
    /// Re-enable a disabled identity.
    Enable {
        /// Email or identity ID.
        target: String,
    },
    /// Set password for an identity.
    SetPassword {
        /// Email or identity ID.
        target: String,
        /// New password. If omitted, reads from stdin.
        password: Option<String>,
    },
    /// Onboard new user (create + welcome email).
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
    Offboard {
        /// Email or identity ID.
        target: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum ProjectAction {
    /// Build the project.
    Build(ProjectRunArgs),
    /// Run tests.
    Test(ProjectRunArgs),
    /// Lint.
    Lint(ProjectRunArgs),
    /// Format.
    Fmt(ProjectRunArgs),
    /// Package (container image, tarball, etc.).
    Package(ProjectRunArgs),
    /// Deploy.
    Deploy(ProjectRunArgs),
    /// Run dev server.
    Dev(ProjectRunArgs),
    /// Clean build artifacts.
    Clean(ProjectRunArgs),
    /// Generate docs.
    Doc(ProjectRunArgs),
    /// Print resolved project config.
    Info,
    /// Print topologically-sorted build order for a verb.
    Order {
        #[arg(default_value = "build")]
        verb: String,
    },
    /// Run a custom verb defined in this project's `targets`.
    Run {
        /// Verb name (must exist in `targets:` of the autodiscovered sunbeam.yaml).
        verb: String,
        #[command(flatten)]
        args: ProjectRunArgs,
    },
    /// Print the dependency DAG as a tree.
    Graph {
        /// Render every owned project's tree (otherwise just the current project).
        #[arg(long)]
        all: bool,
    },
    /// Validate sunbeam.yaml — checks workspace refs, dep cycles, and reports issues.
    Check {
        /// Validate every owned project (otherwise just the current project).
        #[arg(long)]
        all: bool,
    },
}

#[derive(clap::Args, Debug, Clone, Default)]
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
    pub verbose: bool,
    /// Print what would run, don't execute.
    #[arg(long)]
    pub dry_run: bool,
}

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
    /// Per-branch git worktree lifecycle (alias: `sunbeam wt`).
    Worktree {
        #[command(subcommand)]
        action: WorktreeAction,
    },
    /// Print resolved workspace config.
    Info,
    /// List all repos in the workspace (by bucket).
    Repos,
}

#[derive(Subcommand, Debug)]
pub enum WorktreeAction {
    /// Create a new worktree rooted at `.worktrees/<sanitized-branch>`.
    New {
        /// Branch name (will be sanitized for the filesystem path).
        branch: String,
        /// Base ref to branch from (default: current HEAD).
        #[arg(long)]
        from: Option<String>,
        /// Skip running `.hooks/setup` after creation.
        #[arg(long)]
        no_setup: bool,
    },
    /// List all worktrees with dirty-file counts.
    List,
    /// Merge a worktree branch into the current HEAD.
    ///
    /// Default strategy: rebase the branch onto current HEAD, then fast-forward
    /// HEAD to the rebased tip (preserves linear history). Must be run from
    /// the main checkout (not from inside a worktree).
    Merge {
        /// Branch to merge.
        branch: String,
        /// Use classic `git merge` (creates a merge commit instead of rebasing).
        #[arg(long, conflicts_with = "squash")]
        merge_commit: bool,
        /// Use `git merge --squash` (single squashed commit, no rebase).
        #[arg(long, conflicts_with = "merge_commit")]
        squash: bool,
    },
    /// Rebase a branch onto current HEAD (or `--onto <ref>`) without merging.
    ///
    /// Useful for keeping a feature branch current with mainline. Operates in
    /// the branch's worktree if one exists.
    Rebase {
        /// Branch to rebase.
        branch: String,
        /// Rebase onto this ref (default: current HEAD of the main checkout).
        #[arg(long)]
        onto: Option<String>,
    },
    /// Remove a worktree (and optionally delete its branch).
    Rm {
        /// Branch whose worktree should be removed.
        branch: String,
        /// Force removal even if the worktree is dirty.
        #[arg(long)]
        force: bool,
        /// Also delete the local branch after removing the worktree.
        #[arg(long)]
        prune_branch: bool,
    },
    /// Re-run `.hooks/setup` for a named worktree (or the current cwd).
    Setup {
        /// Branch whose worktree to run setup in (defaults to cwd).
        branch: Option<String>,
    },
    /// Drop into an interactive shell inside the worktree.
    ///
    /// Spawns `$SHELL` with its cwd set to the worktree path. Exiting that
    /// shell returns you to your original cwd. Aliases: `enter`, `cd`, `shell`.
    ///
    /// To `cd` in the *current* shell instead of spawning a subshell, source
    /// the wrapper from `sunbeam wt shell-init <shell>` — that turns this
    /// subcommand into a function that does a real `cd`.
    #[command(alias = "enter", alias = "shell")]
    Use {
        /// Branch whose worktree to enter.
        branch: String,
    },
    /// Print the absolute path of a worktree (for shell wrappers / scripts).
    Path {
        /// Branch whose worktree path to print.
        branch: String,
    },
    /// Emit a shell init script that makes `sunbeam wt use` `cd` in the
    /// current shell instead of spawning a subshell. Source from your rc:
    /// `eval "$(sunbeam wt shell-init zsh)"`.
    ShellInit {
        /// Shell flavor.
        #[arg(value_enum)]
        shell: WtShell,
    },
    /// Install the shell integration into your rc file (idempotent).
    ///
    /// Auto-detects shell from `$SHELL`, picks the matching rc file
    /// (`~/.zshrc`, `~/.bashrc`, `~/.config/fish/config.fish`), and appends
    /// the eval line if not already present. Re-run safely.
    ShellInstall {
        /// Shell flavor (default: detect from `$SHELL`).
        #[arg(long, value_enum)]
        shell: Option<WtShell>,
        /// Override the rc file path.
        #[arg(long)]
        rc_file: Option<std::path::PathBuf>,
        /// Print what would be added without writing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Cherry-pick commits from another worktree's branch into the destination.
    ///
    /// Default destination is the current cwd's worktree; override with
    /// `--into <branch>`. Worktrees share `.git`, so any branch is reachable
    /// by name — no fetch needed. Refs starting with `~` or `^` and ranges
    /// without an explicit branch prefix are expanded against `--from`
    /// (`~3..` → `<from>~3..<from>`, `~3..~1` → `<from>~3..<from>~1`).
    ///
    /// Always passes `--signoff` to `git cherry-pick`.
    #[command(name = "cherry-pick", alias = "pick")]
    CherryPick {
        /// Source branch to pick commits from.
        #[arg(long)]
        from: String,
        /// Commit refs or ranges (e.g. `abc123`, `~3..`, `<from>~5..<from>~2`).
        #[arg(required = true)]
        refs: Vec<String>,
        /// Destination worktree branch (default: current cwd's worktree).
        #[arg(long)]
        into: Option<String>,
        /// Edit each commit message before committing (`-e`).
        #[arg(short = 'e', long)]
        edit: bool,
        /// Stage changes but don't commit (`-n`).
        #[arg(short = 'n', long)]
        no_commit: bool,
        /// Append `(cherry picked from commit ...)` to the message (`-x`).
        #[arg(short = 'x')]
        annotate: bool,
        /// Mainline parent number when picking a merge commit.
        #[arg(short = 'm', long)]
        mainline: Option<u32>,
        /// Allow picking into a dirty destination worktree.
        #[arg(long)]
        force: bool,
    },
}

#[derive(clap::ValueEnum, Debug, Clone, Copy)]
pub enum WtShell {
    Bash,
    Zsh,
    Fish,
}

#[derive(Subcommand, Debug)]
pub enum ComposeAction {
    /// Generate and materialize docker-compose.yaml under .sunbeam/compose/.
    Render,
    /// Bring up shared services.
    Up {
        /// Only bring up the named services (default: all).
        services: Vec<String>,
        /// Wait for healthy.
        #[arg(long, default_value_t = true)]
        wait: bool,
    },
    /// Tear down shared services.
    Down {
        /// Also remove volumes.
        #[arg(long)]
        volumes: bool,
    },
    /// List running services.
    Ps,
    /// Tail logs for a service.
    Logs {
        service: String,
        #[arg(short, long)]
        follow: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum StackAction {
    /// List pinned stacks.
    List,
    /// Pin current HEAD SHAs into a named stack.
    Pin {
        name: String,
        /// Only pin the named projects (default: all owned).
        projects: Vec<String>,
        #[arg(long)]
        description: Option<String>,
    },
    /// Checkout the SHAs recorded in a stack.
    Apply {
        name: String,
    },
    /// Diff two stacks (or a stack vs current HEAD).
    Diff {
        left: String,
        right: Option<String>,
    },
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
pub async fn dispatch() -> Result<()> {
    let cli = Cli::parse();

    // Resolve the active context from config + CLI flags (like kubectl)
    let config = crate::config::load_config();
    let active = crate::config::resolve_context(&config, "", cli.context.as_deref(), &cli.domain);

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

        Some(Verb::Up) => {
            crate::output::step("Bringing up cluster (workflow engine)...");

            let ctx_name = {
                let cfg = crate::config::load_config();
                if cfg.current_context.is_empty() {
                    "default".to_string()
                } else {
                    cfg.current_context.clone()
                }
            };

            let host = crate::workflows::host::create_host(&ctx_name).await?;
            crate::workflows::up::register(&host).await;

            let step_ctx = crate::workflows::StepContext::from_active();
            let initial_data = serde_json::json!({
                "__ctx": step_ctx,
                "domain": "",
            });

            let instance = wfe::run_workflow_sync(
                &host,
                "up",
                2,
                initial_data,
                std::time::Duration::from_secs(3600),
            )
            .await
            .map_err(|e| SunbeamError::Other(format!("up workflow failed: {e}")))?;

            crate::workflows::up::print_summary(&instance);
            crate::workflows::host::shutdown_host(host).await;

            if instance.status != wfe_core::models::WorkflowStatus::Complete {
                return Err(SunbeamError::Other(format!(
                    "up workflow ended with status {:?}",
                    instance.status
                )));
            }

            Ok(())
        }

        Some(Verb::Service { action }) => {
            crate::service_cmds::dispatch(action, &cli.domain, &cli.email).await
        }

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
                    crate::output::warn(&format!(
                        "Context '{name}' does not exist. Creating empty context."
                    ));
                    config
                        .contexts
                        .insert(name.clone(), crate::config::Context::default());
                }
                config.current_context = name.clone();
                crate::config::save_config(&config)?;
                crate::output::ok(&format!("Switched to context '{name}'."));
                Ok(())
            }
            Some(ConfigAction::Get) => {
                let config = crate::config::load_config();
                let current = if config.current_context.is_empty() {
                    "(none)"
                } else {
                    &config.current_context
                };
                crate::output::ok(&format!("Current context: {current}"));
                println!();
                for (name, ctx) in &config.contexts {
                    let marker = if name == current { " *" } else { "" };
                    crate::output::ok(&format!("Context: {name}{marker}"));
                    if !ctx.domain.is_empty() {
                        crate::output::ok(&format!("  domain:       {}", ctx.domain));
                    }
                    if !ctx.kube_context.is_empty() {
                        crate::output::ok(&format!("  kube-context: {}", ctx.kube_context));
                    }
                    if !ctx.infra_dir.is_empty() {
                        crate::output::ok(&format!("  infra-dir:    {}", ctx.infra_dir));
                    }
                    if !ctx.acme_email.is_empty() {
                        crate::output::ok(&format!("  acme-email:   {}", ctx.acme_email));
                    }
                    println!();
                }
                Ok(())
            }
            Some(ConfigAction::Clear) => crate::config::clear_config(),
        },

        Some(Verb::Bao { bao_args }) => crate::kube::cmd_bao(&bao_args).await,

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
            Some(AuthAction::Login { domain }) => {
                crate::auth::cmd_auth_login_all(domain.as_deref()).await
            }
            Some(AuthAction::Sso { domain }) => {
                crate::auth::cmd_auth_sso_login(domain.as_deref()).await
            }
            Some(AuthAction::Git { domain }) => {
                crate::auth::cmd_auth_git_login(domain.as_deref()).await
            }
            Some(AuthAction::Logout) => crate::auth::cmd_auth_logout().await,
            Some(AuthAction::Status) => crate::auth::cmd_auth_status().await,
            Some(AuthAction::Token) => crate::auth::cmd_auth_token().await,
        },

        Some(Verb::Pm { action }) => match action {
            None => {
                use clap::CommandFactory;
                let mut cmd = Cli::command();
                let sub = cmd.find_subcommand_mut("pm").expect("pm subcommand");
                sub.print_help()?;
                println!();
                Ok(())
            }
            Some(PmAction::List { source, state }) => {
                let src = if source == "all" {
                    None
                } else {
                    Some(source.as_str())
                };
                crate::pm::cmd_pm_list(src, &state).await
            }
            Some(PmAction::Show { id }) => crate::pm::cmd_pm_show(&id).await,
            Some(PmAction::Create {
                title,
                body,
                source,
                target,
            }) => crate::pm::cmd_pm_create(&title, &body, &source, &target).await,
            Some(PmAction::Comment { id, text }) => crate::pm::cmd_pm_comment(&id, &text).await,
            Some(PmAction::Close { id }) => crate::pm::cmd_pm_close(&id).await,
            Some(PmAction::Assign { id, user }) => crate::pm::cmd_pm_assign(&id, &user).await,
        },

        Some(Verb::Workflow { action }) => {
            let ctx_name = {
                let cfg = crate::config::load_config();
                if cfg.current_context.is_empty() {
                    "default".to_string()
                } else {
                    cfg.current_context.clone()
                }
            };
            crate::workflows::cmd::dispatch(&ctx_name, action).await
        }

        Some(Verb::Workflows { output, action }) => {
            let domain = crate::config::domain();
            if domain.is_empty() {
                return Err(SunbeamError::Config(
                    "domain not set — run `sunbeam config set --domain <domain>` first".into(),
                ));
            }
            if let Err(e) = crate::wfectl::dispatch(action, output, domain).await {
                eprintln!("error: {e:#}");
                std::process::exit(1);
            }
            Ok(())
        }

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
            } => crate::vpn_cmds::cmd_vpn_create_key(&user, user_id, reusable, ephemeral, &expiration).await,
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

        Some(Verb::Doctor) => crate::doctor::cmd_doctor().await,

        Some(Verb::VpnDaemon) => crate::vpn_cmds::cmd_vpn_daemon().await,

        Some(Verb::Update) => crate::update::cmd_update().await,

        Some(Verb::Version) => {
            crate::update::cmd_version();
            Ok(())
        }

        Some(Verb::Project { action }) => crate::project::cli::dispatch(action).await,

        Some(Verb::Operations { action }) => crate::operations::cli::dispatch(action).await,

        Some(Verb::Wt { action }) => crate::operations::cli::dispatch_worktree(action).await,

        Some(Verb::Build { args }) => crate::project::cli::dispatch(ProjectAction::Build(args)).await,
        Some(Verb::Test { args }) => crate::project::cli::dispatch(ProjectAction::Test(args)).await,
        Some(Verb::Lint { args }) => crate::project::cli::dispatch(ProjectAction::Lint(args)).await,
        Some(Verb::Fmt { args }) => crate::project::cli::dispatch(ProjectAction::Fmt(args)).await,
        Some(Verb::Package { args }) => crate::project::cli::dispatch(ProjectAction::Package(args)).await,
        Some(Verb::Deploy { args }) => crate::project::cli::dispatch(ProjectAction::Deploy(args)).await,
        Some(Verb::Dev { args }) => crate::project::cli::dispatch(ProjectAction::Dev(args)).await,
        Some(Verb::Clean { args }) => crate::project::cli::dispatch(ProjectAction::Clean(args)).await,
        Some(Verb::Doc { args }) => crate::project::cli::dispatch(ProjectAction::Doc(args)).await,
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
        assert!(matches!(cli.verb, Some(Verb::Up)));
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
                action: ServiceAction::Apply {
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
                action: ServiceAction::Apply {
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
                action: ServiceAction::Apply {
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
            Some(Verb::Workflow { action }) => {
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
            Some(Verb::Workflow { action }) => match action {
                crate::workflows::cmd::WorkflowAction::List { status } => {
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
            Some(Verb::Workflow { action }) => match action {
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
            Some(Verb::Workflow { action }) => match action {
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
            Some(Verb::Workflow { action }) => match action {
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
            Some(Verb::Workflow { action }) => match action {
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
            Some(Verb::Workflow { action }) => match action {
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
                action: ServiceAction::Deploy { target, all },
            }) => {
                assert!(target.is_none());
                assert!(!all);
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
                action: OperationsAction::Compose {
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
                action: OperationsAction::Compose {
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
                action: OperationsAction::Stack {
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
    fn test_top_level_build_alias() {
        let cli = parse(&["sunbeam", "build"]);
        match cli.verb {
            Some(Verb::Build { args }) => {
                assert!(!args.all);
                assert!(args.projects.is_empty());
                assert!(!args.with_deps);
                assert!(args.jobs.is_none());
            }
            _ => panic!("expected top-level Build alias"),
        }
    }

    #[test]
    fn test_top_level_test_alias_with_flags() {
        let cli = parse(&["sunbeam", "test", "--all", "--dry-run", "--jobs", "4"]);
        match cli.verb {
            Some(Verb::Test { args }) => {
                assert!(args.all);
                assert!(args.dry_run);
                assert_eq!(args.jobs, Some(4));
            }
            _ => panic!("expected top-level Test alias"),
        }
    }

    #[test]
    fn test_top_level_aliases_all_present() {
        for verb in [
            "build", "test", "lint", "fmt", "package", "deploy", "dev", "clean", "doc",
        ] {
            let cli = parse(&["sunbeam", verb]);
            assert!(
                matches!(
                    cli.verb,
                    Some(Verb::Build { .. })
                        | Some(Verb::Test { .. })
                        | Some(Verb::Lint { .. })
                        | Some(Verb::Fmt { .. })
                        | Some(Verb::Package { .. })
                        | Some(Verb::Deploy { .. })
                        | Some(Verb::Dev { .. })
                        | Some(Verb::Clean { .. })
                        | Some(Verb::Doc { .. })
                ),
                "verb {verb} did not parse to a top-level alias"
            );
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

    #[test]
    fn test_with_deps_flag() {
        let cli = parse(&[
            "sunbeam", "build", "-p", "sol", "--with-deps",
        ]);
        match cli.verb {
            Some(Verb::Build { args }) => {
                assert_eq!(args.projects, vec!["sol"]);
                assert!(args.with_deps);
            }
            _ => panic!("expected Build with --with-deps"),
        }
    }
}
