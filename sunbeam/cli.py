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
    p_build.add_argument("what", choices=["proxy"],
                         help="What to build (proxy)")

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

    else:
        parser.print_help()
        sys.exit(1)
