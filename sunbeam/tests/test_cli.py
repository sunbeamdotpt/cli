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
        sub.add_parser("apply")
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
        p_build.add_argument("what", choices=["proxy"])
        sub.add_parser("mirror")
        sub.add_parser("bootstrap")
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

    def test_build_invalid_target(self):
        with self.assertRaises(SystemExit):
            self._parse(["build", "notavalidtarget"])

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
        mock_build.assert_called_once_with("proxy")
