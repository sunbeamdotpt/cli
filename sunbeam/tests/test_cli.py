"""Tests for CLI routing and argument validation."""
import sys
import unittest
from unittest.mock import MagicMock, patch
import argparse


class TestArgParsing(unittest.TestCase):
    """Test that argparse parses arguments correctly."""

    def _parse(self, argv):
        """Parse argv using the same parser as main(), return args namespace."""
        parser = argparse.ArgumentParser(prog="sunbeam")
        sub = parser.add_subparsers(dest="verb", metavar="verb")
        sub.add_parser("up")
        sub.add_parser("down")
        p_status = sub.add_parser("status")
        p_status.add_argument("target", nargs="?", default=None)
        p_apply = sub.add_parser("apply")
        p_apply.add_argument("namespace", nargs="?", default="")
        p_apply.add_argument("--domain", default="")
        p_apply.add_argument("--email", default="")
        sub.add_parser("seed")
        sub.add_parser("verify")
        p_logs = sub.add_parser("logs")
        p_logs.add_argument("target")
        p_logs.add_argument("-f", "--follow", action="store_true")
        p_get = sub.add_parser("get")
        p_get.add_argument("target")
        p_get.add_argument("-o", "--output", default="yaml", choices=["yaml", "json", "wide"])
        p_restart = sub.add_parser("restart")
        p_restart.add_argument("target", nargs="?", default=None)
        p_build = sub.add_parser("build")
        p_build.add_argument("what", choices=["proxy", "integration", "kratos-admin", "meet",
                                               "docs-frontend", "people-frontend"])
        p_build.add_argument("--push", action="store_true")
        p_build.add_argument("--deploy", action="store_true")
        sub.add_parser("mirror")
        sub.add_parser("bootstrap")
        p_check = sub.add_parser("check")
        p_check.add_argument("target", nargs="?", default=None)
        p_user = sub.add_parser("user")
        user_sub = p_user.add_subparsers(dest="user_action")
        p_user_list = user_sub.add_parser("list")
        p_user_list.add_argument("--search", default="")
        p_user_get = user_sub.add_parser("get")
        p_user_get.add_argument("target")
        p_user_create = user_sub.add_parser("create")
        p_user_create.add_argument("email")
        p_user_create.add_argument("--name", default="")
        p_user_create.add_argument("--schema", default="default")
        p_user_delete = user_sub.add_parser("delete")
        p_user_delete.add_argument("target")
        p_user_recover = user_sub.add_parser("recover")
        p_user_recover.add_argument("target")
        p_user_disable = user_sub.add_parser("disable")
        p_user_disable.add_argument("target")
        p_user_enable = user_sub.add_parser("enable")
        p_user_enable.add_argument("target")
        p_user_set_pw = user_sub.add_parser("set-password")
        p_user_set_pw.add_argument("target")
        p_user_set_pw.add_argument("password")
        return parser.parse_args(argv)

    def test_up(self):
        args = self._parse(["up"])
        self.assertEqual(args.verb, "up")

    def test_status_no_target(self):
        args = self._parse(["status"])
        self.assertEqual(args.verb, "status")
        self.assertIsNone(args.target)

    def test_status_with_namespace(self):
        args = self._parse(["status", "ory"])
        self.assertEqual(args.verb, "status")
        self.assertEqual(args.target, "ory")

    def test_logs_no_follow(self):
        args = self._parse(["logs", "ory/kratos"])
        self.assertEqual(args.verb, "logs")
        self.assertEqual(args.target, "ory/kratos")
        self.assertFalse(args.follow)

    def test_logs_follow_short(self):
        args = self._parse(["logs", "ory/kratos", "-f"])
        self.assertTrue(args.follow)

    def test_logs_follow_long(self):
        args = self._parse(["logs", "ory/kratos", "--follow"])
        self.assertTrue(args.follow)

    def test_build_proxy(self):
        args = self._parse(["build", "proxy"])
        self.assertEqual(args.what, "proxy")
        self.assertFalse(args.push)
        self.assertFalse(args.deploy)

    def test_build_integration(self):
        args = self._parse(["build", "integration"])
        self.assertEqual(args.what, "integration")

    def test_build_push_flag(self):
        args = self._parse(["build", "proxy", "--push"])
        self.assertTrue(args.push)
        self.assertFalse(args.deploy)

    def test_build_deploy_flag(self):
        args = self._parse(["build", "proxy", "--deploy"])
        self.assertFalse(args.push)
        self.assertTrue(args.deploy)

    def test_build_invalid_target(self):
        with self.assertRaises(SystemExit):
            self._parse(["build", "notavalidtarget"])

    def test_user_set_password(self):
        args = self._parse(["user", "set-password", "admin@example.com", "hunter2"])
        self.assertEqual(args.verb, "user")
        self.assertEqual(args.user_action, "set-password")
        self.assertEqual(args.target, "admin@example.com")
        self.assertEqual(args.password, "hunter2")

    def test_user_disable(self):
        args = self._parse(["user", "disable", "admin@example.com"])
        self.assertEqual(args.user_action, "disable")
        self.assertEqual(args.target, "admin@example.com")

    def test_user_enable(self):
        args = self._parse(["user", "enable", "admin@example.com"])
        self.assertEqual(args.user_action, "enable")
        self.assertEqual(args.target, "admin@example.com")

    def test_user_list_search(self):
        args = self._parse(["user", "list", "--search", "sienna"])
        self.assertEqual(args.user_action, "list")
        self.assertEqual(args.search, "sienna")

    def test_user_create(self):
        args = self._parse(["user", "create", "x@example.com", "--name", "X Y"])
        self.assertEqual(args.user_action, "create")
        self.assertEqual(args.email, "x@example.com")
        self.assertEqual(args.name, "X Y")

    def test_get_with_target(self):
        args = self._parse(["get", "ory/kratos-abc"])
        self.assertEqual(args.verb, "get")
        self.assertEqual(args.target, "ory/kratos-abc")
        self.assertEqual(args.output, "yaml")

    def test_get_json_output(self):
        args = self._parse(["get", "ory/kratos-abc", "-o", "json"])
        self.assertEqual(args.output, "json")

    def test_get_invalid_output_format(self):
        with self.assertRaises(SystemExit):
            self._parse(["get", "ory/kratos-abc", "-o", "toml"])

    def test_check_no_target(self):
        args = self._parse(["check"])
        self.assertEqual(args.verb, "check")
        self.assertIsNone(args.target)

    def test_check_with_namespace(self):
        args = self._parse(["check", "devtools"])
        self.assertEqual(args.verb, "check")
        self.assertEqual(args.target, "devtools")

    def test_check_with_service(self):
        args = self._parse(["check", "lasuite/people"])
        self.assertEqual(args.verb, "check")
        self.assertEqual(args.target, "lasuite/people")

    def test_apply_no_namespace(self):
        args = self._parse(["apply"])
        self.assertEqual(args.verb, "apply")
        self.assertEqual(args.namespace, "")

    def test_apply_with_namespace(self):
        args = self._parse(["apply", "lasuite"])
        self.assertEqual(args.verb, "apply")
        self.assertEqual(args.namespace, "lasuite")

    def test_apply_ingress_namespace(self):
        args = self._parse(["apply", "ingress"])
        self.assertEqual(args.namespace, "ingress")

    def test_build_meet(self):
        args = self._parse(["build", "meet"])
        self.assertEqual(args.what, "meet")

    def test_no_args_verb_is_none(self):
        args = self._parse([])
        self.assertIsNone(args.verb)


class TestCliDispatch(unittest.TestCase):
    """Test that main() dispatches to the correct command function."""

    def test_no_verb_exits_0(self):
        with patch.object(sys, "argv", ["sunbeam"]):
            from sunbeam import cli
            with self.assertRaises(SystemExit) as ctx:
                cli.main()
            self.assertEqual(ctx.exception.code, 0)

    def test_unknown_verb_exits_nonzero(self):
        with patch.object(sys, "argv", ["sunbeam", "unknown-verb"]):
            from sunbeam import cli
            with self.assertRaises(SystemExit) as ctx:
                cli.main()
            self.assertNotEqual(ctx.exception.code, 0)

    def test_up_calls_cmd_up(self):
        mock_up = MagicMock()
        with patch.object(sys, "argv", ["sunbeam", "up"]):
            with patch.dict("sys.modules", {"sunbeam.cluster": MagicMock(cmd_up=mock_up)}):
                import importlib
                import sunbeam.cli as cli_mod
                importlib.reload(cli_mod)
                try:
                    cli_mod.main()
                except SystemExit:
                    pass
        mock_up.assert_called_once()

    def test_status_no_target(self):
        mock_status = MagicMock()
        with patch.object(sys, "argv", ["sunbeam", "status"]):
            with patch.dict("sys.modules", {"sunbeam.services": MagicMock(cmd_status=mock_status)}):
                import importlib, sunbeam.cli as cli_mod
                importlib.reload(cli_mod)
                try:
                    cli_mod.main()
                except SystemExit:
                    pass
        mock_status.assert_called_once_with(None)

    def test_status_with_namespace(self):
        mock_status = MagicMock()
        with patch.object(sys, "argv", ["sunbeam", "status", "ory"]):
            with patch.dict("sys.modules", {"sunbeam.services": MagicMock(cmd_status=mock_status)}):
                import importlib, sunbeam.cli as cli_mod
                importlib.reload(cli_mod)
                try:
                    cli_mod.main()
                except SystemExit:
                    pass
        mock_status.assert_called_once_with("ory")

    def test_logs_with_target(self):
        mock_logs = MagicMock()
        with patch.object(sys, "argv", ["sunbeam", "logs", "ory/kratos"]):
            with patch.dict("sys.modules", {"sunbeam.services": MagicMock(cmd_logs=mock_logs)}):
                import importlib, sunbeam.cli as cli_mod
                importlib.reload(cli_mod)
                try:
                    cli_mod.main()
                except SystemExit:
                    pass
        mock_logs.assert_called_once_with("ory/kratos", follow=False)

    def test_logs_follow_flag(self):
        mock_logs = MagicMock()
        with patch.object(sys, "argv", ["sunbeam", "logs", "ory/kratos", "-f"]):
            with patch.dict("sys.modules", {"sunbeam.services": MagicMock(cmd_logs=mock_logs)}):
                import importlib, sunbeam.cli as cli_mod
                importlib.reload(cli_mod)
                try:
                    cli_mod.main()
                except SystemExit:
                    pass
        mock_logs.assert_called_once_with("ory/kratos", follow=True)

    def test_get_dispatches_with_target_and_output(self):
        mock_get = MagicMock()
        with patch.object(sys, "argv", ["sunbeam", "get", "ory/kratos-abc"]):
            with patch.dict("sys.modules", {"sunbeam.services": MagicMock(cmd_get=mock_get)}):
                import importlib, sunbeam.cli as cli_mod
                importlib.reload(cli_mod)
                try:
                    cli_mod.main()
                except SystemExit:
                    pass
        mock_get.assert_called_once_with("ory/kratos-abc", output="yaml")

    def test_build_proxy(self):
        mock_build = MagicMock()
        with patch.object(sys, "argv", ["sunbeam", "build", "proxy"]):
            with patch.dict("sys.modules", {"sunbeam.images": MagicMock(cmd_build=mock_build)}):
                import importlib, sunbeam.cli as cli_mod
                importlib.reload(cli_mod)
                try:
                    cli_mod.main()
                except SystemExit:
                    pass
        mock_build.assert_called_once_with("proxy", push=False, deploy=False)

    def test_build_with_push_flag(self):
        mock_build = MagicMock()
        with patch.object(sys, "argv", ["sunbeam", "build", "integration", "--push"]):
            with patch.dict("sys.modules", {"sunbeam.images": MagicMock(cmd_build=mock_build)}):
                import importlib, sunbeam.cli as cli_mod
                importlib.reload(cli_mod)
                try:
                    cli_mod.main()
                except SystemExit:
                    pass
        mock_build.assert_called_once_with("integration", push=True, deploy=False)

    def test_build_with_deploy_flag_implies_push(self):
        mock_build = MagicMock()
        with patch.object(sys, "argv", ["sunbeam", "build", "proxy", "--deploy"]):
            with patch.dict("sys.modules", {"sunbeam.images": MagicMock(cmd_build=mock_build)}):
                import importlib, sunbeam.cli as cli_mod
                importlib.reload(cli_mod)
                try:
                    cli_mod.main()
                except SystemExit:
                    pass
        mock_build.assert_called_once_with("proxy", push=True, deploy=True)

    def test_user_set_password_dispatches(self):
        mock_set_pw = MagicMock()
        mock_users = MagicMock(
            cmd_user_list=MagicMock(), cmd_user_get=MagicMock(),
            cmd_user_create=MagicMock(), cmd_user_delete=MagicMock(),
            cmd_user_recover=MagicMock(), cmd_user_disable=MagicMock(),
            cmd_user_enable=MagicMock(), cmd_user_set_password=mock_set_pw,
        )
        with patch.object(sys, "argv", ["sunbeam", "user", "set-password",
                                        "admin@sunbeam.pt", "s3cr3t"]):
            with patch.dict("sys.modules", {"sunbeam.users": mock_users}):
                import importlib, sunbeam.cli as cli_mod
                importlib.reload(cli_mod)
                try:
                    cli_mod.main()
                except SystemExit:
                    pass
        mock_set_pw.assert_called_once_with("admin@sunbeam.pt", "s3cr3t")

    def test_user_disable_dispatches(self):
        mock_disable = MagicMock()
        mock_users = MagicMock(
            cmd_user_list=MagicMock(), cmd_user_get=MagicMock(),
            cmd_user_create=MagicMock(), cmd_user_delete=MagicMock(),
            cmd_user_recover=MagicMock(), cmd_user_disable=mock_disable,
            cmd_user_enable=MagicMock(), cmd_user_set_password=MagicMock(),
        )
        with patch.object(sys, "argv", ["sunbeam", "user", "disable", "x@sunbeam.pt"]):
            with patch.dict("sys.modules", {"sunbeam.users": mock_users}):
                import importlib, sunbeam.cli as cli_mod
                importlib.reload(cli_mod)
                try:
                    cli_mod.main()
                except SystemExit:
                    pass
        mock_disable.assert_called_once_with("x@sunbeam.pt")

    def test_user_enable_dispatches(self):
        mock_enable = MagicMock()
        mock_users = MagicMock(
            cmd_user_list=MagicMock(), cmd_user_get=MagicMock(),
            cmd_user_create=MagicMock(), cmd_user_delete=MagicMock(),
            cmd_user_recover=MagicMock(), cmd_user_disable=MagicMock(),
            cmd_user_enable=mock_enable, cmd_user_set_password=MagicMock(),
        )
        with patch.object(sys, "argv", ["sunbeam", "user", "enable", "x@sunbeam.pt"]):
            with patch.dict("sys.modules", {"sunbeam.users": mock_users}):
                import importlib, sunbeam.cli as cli_mod
                importlib.reload(cli_mod)
                try:
                    cli_mod.main()
                except SystemExit:
                    pass
        mock_enable.assert_called_once_with("x@sunbeam.pt")

    def test_apply_full_dispatches_without_namespace(self):
        mock_apply = MagicMock()
        with patch.object(sys, "argv", ["sunbeam", "apply"]):
            with patch.dict("sys.modules", {"sunbeam.manifests": MagicMock(cmd_apply=mock_apply)}):
                import importlib, sunbeam.cli as cli_mod
                importlib.reload(cli_mod)
                try:
                    cli_mod.main()
                except SystemExit:
                    pass
        mock_apply.assert_called_once_with(env="local", domain="", email="", namespace="")

    def test_apply_partial_passes_namespace(self):
        mock_apply = MagicMock()
        with patch.object(sys, "argv", ["sunbeam", "apply", "lasuite"]):
            with patch.dict("sys.modules", {"sunbeam.manifests": MagicMock(cmd_apply=mock_apply)}):
                import importlib, sunbeam.cli as cli_mod
                importlib.reload(cli_mod)
                try:
                    cli_mod.main()
                except SystemExit:
                    pass
        mock_apply.assert_called_once_with(env="local", domain="", email="", namespace="lasuite")

    def test_build_meet_dispatches(self):
        mock_build = MagicMock()
        with patch.object(sys, "argv", ["sunbeam", "build", "meet"]):
            with patch.dict("sys.modules", {"sunbeam.images": MagicMock(cmd_build=mock_build)}):
                import importlib, sunbeam.cli as cli_mod
                importlib.reload(cli_mod)
                try:
                    cli_mod.main()
                except SystemExit:
                    pass
        mock_build.assert_called_once_with("meet", push=False, deploy=False)

    def test_check_no_target(self):
        mock_check = MagicMock()
        with patch.object(sys, "argv", ["sunbeam", "check"]):
            with patch.dict("sys.modules", {"sunbeam.checks": MagicMock(cmd_check=mock_check)}):
                import importlib, sunbeam.cli as cli_mod
                importlib.reload(cli_mod)
                try:
                    cli_mod.main()
                except SystemExit:
                    pass
        mock_check.assert_called_once_with(None)

    def test_check_with_target(self):
        mock_check = MagicMock()
        with patch.object(sys, "argv", ["sunbeam", "check", "lasuite/people"]):
            with patch.dict("sys.modules", {"sunbeam.checks": MagicMock(cmd_check=mock_check)}):
                import importlib, sunbeam.cli as cli_mod
                importlib.reload(cli_mod)
                try:
                    cli_mod.main()
                except SystemExit:
                    pass
        mock_check.assert_called_once_with("lasuite/people")
