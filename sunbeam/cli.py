"""CLI entry point — argparse dispatch table for all sunbeam verbs."""
import argparse
import sys


ENV_CONTEXTS = {
    "local":      "sunbeam",
    "production": "production",
}


def main() -> None:
    parser = argparse.ArgumentParser(
        prog="sunbeam",
        description="Sunbeam local dev stack manager",
    )
    parser.add_argument(
        "--env", choices=["local", "production"], default="local",
        help="Target environment (default: local)",
    )
    parser.add_argument(
        "--context", default=None,
        help="kubectl context override (default: sunbeam for local, default for production)",
    )
    parser.add_argument(
        "--domain", default="",
        help="Domain suffix for production deploys (e.g. sunbeam.pt)",
    )
    parser.add_argument(
        "--email", default="",
        help="ACME email for cert-manager (e.g. ops@sunbeam.pt)",
    )

    sub = parser.add_subparsers(dest="verb", metavar="verb")

    # sunbeam up
    sub.add_parser("up", help="Full cluster bring-up")

    # sunbeam down
    sub.add_parser("down", help="Tear down Lima VM")

    # sunbeam status [ns[/name]]
    p_status = sub.add_parser("status", help="Pod health (optionally scoped)")
    p_status.add_argument("target", nargs="?", default=None,
                          help="namespace or namespace/name")

    # sunbeam apply [namespace]
    p_apply = sub.add_parser("apply", help="kustomize build + domain subst + kubectl apply")
    p_apply.add_argument("namespace", nargs="?", default="",
                         help="Limit apply to one namespace (e.g. lasuite, ingress, ory)")
    p_apply.add_argument("--domain", default="", help="Domain suffix (e.g. sunbeam.pt)")
    p_apply.add_argument("--email", default="", help="ACME email for cert-manager")

    # sunbeam seed
    sub.add_parser("seed", help="Generate/store all credentials in OpenBao")

    # sunbeam verify
    sub.add_parser("verify", help="E2E VSO + OpenBao integration test")

    # sunbeam logs <ns/name> [-f]
    p_logs = sub.add_parser("logs", help="kubectl logs for a service")
    p_logs.add_argument("target", help="namespace/name")
    p_logs.add_argument("-f", "--follow", action="store_true",
                        help="Stream logs (--follow)")

    # sunbeam get <ns/name> [-o yaml|json|wide]
    p_get = sub.add_parser("get", help="Raw kubectl get for a pod (ns/name)")
    p_get.add_argument("target", help="namespace/name")
    p_get.add_argument("-o", "--output", default="yaml",
                       choices=["yaml", "json", "wide"],
                       help="Output format (default: yaml)")

    # sunbeam restart [ns[/name]]
    p_restart = sub.add_parser("restart", help="Rolling restart of services")
    p_restart.add_argument("target", nargs="?", default=None,
                           help="namespace or namespace/name")

    # sunbeam build <what> [--push] [--deploy]
    p_build = sub.add_parser("build", help="Build an artifact (add --push to push, --deploy to apply+rollout)")
    p_build.add_argument("what",
                         choices=["proxy", "integration", "kratos-admin", "meet",
                                  "docs-frontend", "people-frontend", "people",
                                  "messages", "messages-backend", "messages-frontend",
                                  "messages-mta-in", "messages-mta-out",
                                  "messages-mpa", "messages-socks-proxy",
                                  "tuwunel"],
                         help="What to build")
    p_build.add_argument("--push", action="store_true",
                         help="Push image to registry after building")
    p_build.add_argument("--deploy", action="store_true",
                         help="Apply manifests and rollout restart after pushing (implies --push)")

    # sunbeam check [ns[/name]]
    p_check = sub.add_parser("check", help="Functional service health checks")
    p_check.add_argument("target", nargs="?", default=None,
                         help="namespace or namespace/name")

    # sunbeam mirror
    sub.add_parser("mirror", help="Mirror amd64-only La Suite images")

    # sunbeam bootstrap
    sub.add_parser("bootstrap", help="Create Gitea orgs/repos; set up Lima registry")

    # sunbeam config <action> [args]
    p_config = sub.add_parser("config", help="Manage sunbeam configuration")
    config_sub = p_config.add_subparsers(dest="config_action", metavar="action")

    # sunbeam config set --host HOST --infra-dir DIR --acme-email EMAIL
    p_config_set = config_sub.add_parser("set", help="Set configuration values")
    p_config_set.add_argument("--host", default="",
                           help="Production SSH host (e.g. user@server.example.com)")
    p_config_set.add_argument("--infra-dir", default="",
                           help="Infrastructure directory root")
    p_config_set.add_argument("--acme-email", default="",
                           help="ACME email for Let's Encrypt certificates (e.g. ops@sunbeam.pt)")

    # sunbeam config get
    config_sub.add_parser("get", help="Get current configuration")

    # sunbeam config clear
    config_sub.add_parser("clear", help="Clear configuration")

    # sunbeam k8s [kubectl args...] — transparent kubectl --context=sunbeam wrapper
    p_k8s = sub.add_parser("k8s", help="kubectl --context=sunbeam passthrough")
    p_k8s.add_argument("kubectl_args", nargs=argparse.REMAINDER,
                       help="arguments forwarded verbatim to kubectl")

    # sunbeam bao [bao args...] — bao CLI inside OpenBao pod with root token injected
    p_bao = sub.add_parser("bao", help="bao CLI passthrough (runs inside OpenBao pod with root token)")
    p_bao.add_argument("bao_args", nargs=argparse.REMAINDER,
                       help="arguments forwarded verbatim to bao")

    # sunbeam user <action> [args]
    p_user = sub.add_parser("user", help="User/identity management")
    user_sub = p_user.add_subparsers(dest="user_action", metavar="action")

    p_user_list = user_sub.add_parser("list", help="List identities")
    p_user_list.add_argument("--search", default="", help="Filter by email")

    p_user_get = user_sub.add_parser("get", help="Get identity by email or ID")
    p_user_get.add_argument("target", help="Email or identity ID")

    p_user_create = user_sub.add_parser("create", help="Create identity")
    p_user_create.add_argument("email", help="Email address")
    p_user_create.add_argument("--name", default="", help="Display name")
    p_user_create.add_argument("--schema", default="default", help="Schema ID")

    p_user_delete = user_sub.add_parser("delete", help="Delete identity")
    p_user_delete.add_argument("target", help="Email or identity ID")

    p_user_recover = user_sub.add_parser("recover", help="Generate recovery link")
    p_user_recover.add_argument("target", help="Email or identity ID")

    p_user_disable = user_sub.add_parser("disable", help="Disable identity + revoke sessions (lockout)")
    p_user_disable.add_argument("target", help="Email or identity ID")

    p_user_enable = user_sub.add_parser("enable", help="Re-enable a disabled identity")
    p_user_enable.add_argument("target", help="Email or identity ID")

    p_user_set_pw = user_sub.add_parser("set-password", help="Set password for an identity")
    p_user_set_pw.add_argument("target", help="Email or identity ID")
    p_user_set_pw.add_argument("password", help="New password")

    args = parser.parse_args()



    # Set kubectl context before any kube calls.
    # For production, also register the SSH host so the tunnel is opened on demand.
    # SUNBEAM_SSH_HOST env var: e.g. "user@server.example.com" or just "server.example.com"
    import os
    from sunbeam.kube import set_context
    from sunbeam.config import get_production_host
    
    ctx = args.context or ENV_CONTEXTS.get(args.env, "sunbeam")
    ssh_host = ""
    if args.env == "production":
        ssh_host = get_production_host()
        if not ssh_host:
            from sunbeam.output import die
            die("Production host not configured. Use --host to set it or set SUNBEAM_SSH_HOST environment variable.")
    set_context(ctx, ssh_host=ssh_host)

    if args.verb is None:
        parser.print_help()
        sys.exit(0)

    # Lazy imports to keep startup fast
    if args.verb == "up":
        from sunbeam.cluster import cmd_up
        cmd_up()

    elif args.verb == "down":
        from sunbeam.cluster import cmd_down
        cmd_down()

    elif args.verb == "status":
        from sunbeam.services import cmd_status
        cmd_status(args.target)

    elif args.verb == "apply":
        from sunbeam.manifests import cmd_apply
        # --domain/--email can appear before OR after the verb; subparser wins if both set.
        domain    = getattr(args, "domain",    "") or ""
        email     = getattr(args, "email",     "") or ""
        namespace = getattr(args, "namespace", "") or ""
        cmd_apply(env=args.env, domain=domain, email=email, namespace=namespace)

    elif args.verb == "seed":
        from sunbeam.secrets import cmd_seed
        cmd_seed()

    elif args.verb == "verify":
        from sunbeam.secrets import cmd_verify
        cmd_verify()

    elif args.verb == "logs":
        from sunbeam.services import cmd_logs
        cmd_logs(args.target, follow=args.follow)

    elif args.verb == "get":
        from sunbeam.services import cmd_get
        cmd_get(args.target, output=args.output)

    elif args.verb == "restart":
        from sunbeam.services import cmd_restart
        cmd_restart(args.target)

    elif args.verb == "build":
        from sunbeam.images import cmd_build
        push = args.push or args.deploy
        cmd_build(args.what, push=push, deploy=args.deploy)

    elif args.verb == "check":
        from sunbeam.checks import cmd_check
        cmd_check(args.target)

    elif args.verb == "mirror":
        from sunbeam.images import cmd_mirror
        cmd_mirror()

    elif args.verb == "bootstrap":
        from sunbeam.gitea import cmd_bootstrap
        cmd_bootstrap()

    elif args.verb == "config":
        from sunbeam.config import (
            SunbeamConfig, load_config, save_config, get_production_host, get_infra_directory
        )
        action = getattr(args, "config_action", None)
        if action is None:
            p_config.print_help()
            sys.exit(0)
        elif action == "set":
            config = load_config()
            if args.host:
                config.production_host = args.host
            if args.infra_dir:
                config.infra_directory = args.infra_dir
            if args.acme_email:
                config.acme_email = args.acme_email
            save_config(config)
        elif action == "get":
            from sunbeam.output import ok
            config = load_config()
            ok(f"Production host: {config.production_host or '(not set)'}")
            ok(f"Infrastructure directory: {config.infra_directory or '(not set)'}")
            ok(f"ACME email: {config.acme_email or '(not set)'}")

            # Also show effective production host (from config or env)
            effective_host = get_production_host()
            if effective_host:
                ok(f"Effective production host: {effective_host}")
        elif action == "clear":
            import os
            config_path = os.path.expanduser("~/.sunbeam.json")
            if os.path.exists(config_path):
                os.remove(config_path)
                from sunbeam.output import ok
                ok(f"Configuration cleared from {config_path}")
            else:
                from sunbeam.output import warn
                warn("No configuration file found to clear")

    elif args.verb == "k8s":
        from sunbeam.kube import cmd_k8s
        sys.exit(cmd_k8s(args.kubectl_args))

    elif args.verb == "bao":
        from sunbeam.kube import cmd_bao
        sys.exit(cmd_bao(args.bao_args))

    elif args.verb == "user":
        from sunbeam.users import (cmd_user_list, cmd_user_get, cmd_user_create,
                                   cmd_user_delete, cmd_user_recover,
                                   cmd_user_disable, cmd_user_enable,
                                   cmd_user_set_password)
        action = getattr(args, "user_action", None)
        if action is None:
            p_user.print_help()
            sys.exit(0)
        elif action == "list":
            cmd_user_list(search=args.search)
        elif action == "get":
            cmd_user_get(args.target)
        elif action == "create":
            cmd_user_create(args.email, name=args.name, schema_id=args.schema)
        elif action == "delete":
            cmd_user_delete(args.target)
        elif action == "recover":
            cmd_user_recover(args.target)
        elif action == "disable":
            cmd_user_disable(args.target)
        elif action == "enable":
            cmd_user_enable(args.target)
        elif action == "set-password":
            cmd_user_set_password(args.target, args.password)

    else:
        parser.print_help()
        sys.exit(1)
