"""CLI entry point — argparse dispatch table for all sunbeam verbs."""
import argparse
import sys


def main() -> None:
    parser = argparse.ArgumentParser(
        prog="sunbeam",
        description="Sunbeam local dev stack manager",
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

    # sunbeam apply
    sub.add_parser("apply", help="kustomize build + domain subst + kubectl apply")

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

    # sunbeam build <what>
    p_build = sub.add_parser("build", help="Build and push an artifact")
    p_build.add_argument("what", choices=["proxy", "kratos-admin"],
                         help="What to build (proxy, kratos-admin)")

    # sunbeam check [ns[/name]]
    p_check = sub.add_parser("check", help="Functional service health checks")
    p_check.add_argument("target", nargs="?", default=None,
                         help="namespace or namespace/name")

    # sunbeam mirror
    sub.add_parser("mirror", help="Mirror amd64-only La Suite images")

    # sunbeam bootstrap
    sub.add_parser("bootstrap", help="Create Gitea orgs/repos; set up Lima registry")

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

    args = parser.parse_args()

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
        cmd_apply()

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
        cmd_build(args.what)

    elif args.verb == "check":
        from sunbeam.checks import cmd_check
        cmd_check(args.target)

    elif args.verb == "mirror":
        from sunbeam.images import cmd_mirror
        cmd_mirror()

    elif args.verb == "bootstrap":
        from sunbeam.gitea import cmd_bootstrap
        cmd_bootstrap()

    elif args.verb == "k8s":
        from sunbeam.kube import cmd_k8s
        sys.exit(cmd_k8s(args.kubectl_args))

    elif args.verb == "bao":
        from sunbeam.kube import cmd_bao
        sys.exit(cmd_bao(args.bao_args))

    elif args.verb == "user":
        from sunbeam.users import (cmd_user_list, cmd_user_get, cmd_user_create,
                                   cmd_user_delete, cmd_user_recover)
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

    else:
        parser.print_help()
        sys.exit(1)
