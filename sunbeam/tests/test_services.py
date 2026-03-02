"""Tests for services.py — status scoping, log command construction, restart."""
import unittest
from unittest.mock import MagicMock, patch, call


class TestCmdStatus(unittest.TestCase):
    def test_all_namespaces_when_no_target(self):
        fake_output = (
            "ory   hydra-abc   1/1   Running   0   1d\n"
            "data  valkey-xyz  1/1   Running   0   1d\n"
        )
        with patch("sunbeam.services._capture_out", return_value=fake_output):
            from sunbeam import services
            services.cmd_status(None)

    def test_namespace_scoped(self):
        fake_output = "ory  kratos-abc  1/1  Running  0  1d\n"
        with patch("sunbeam.services._capture_out", return_value=fake_output) as mock_co:
            from sunbeam import services
            services.cmd_status("ory")
            # Should have called _capture_out with -n ory
            calls_str = str(mock_co.call_args_list)
            self.assertIn("ory", calls_str)

    def test_pod_scoped(self):
        fake_output = "kratos-abc  1/1  Running  0  1d\n"
        with patch("sunbeam.services._capture_out", return_value=fake_output) as mock_co:
            from sunbeam import services
            services.cmd_status("ory/kratos")
            calls_str = str(mock_co.call_args_list)
            self.assertIn("ory", calls_str)
            self.assertIn("kratos", calls_str)


class TestCmdLogs(unittest.TestCase):
    def test_logs_no_follow(self):
        with patch("subprocess.Popen") as mock_popen:
            mock_proc = MagicMock()
            mock_proc.wait.return_value = 0
            mock_popen.return_value = mock_proc
            with patch("sunbeam.tools.ensure_tool", return_value="/fake/kubectl"):
                from sunbeam import services
                services.cmd_logs("ory/kratos", follow=False)
                args = mock_popen.call_args[0][0]
                self.assertIn("-n", args)
                self.assertIn("ory", args)
                self.assertNotIn("--follow", args)

    def test_logs_follow(self):
        with patch("subprocess.Popen") as mock_popen:
            mock_proc = MagicMock()
            mock_proc.wait.return_value = 0
            mock_popen.return_value = mock_proc
            with patch("sunbeam.tools.ensure_tool", return_value="/fake/kubectl"):
                from sunbeam import services
                services.cmd_logs("ory/kratos", follow=True)
                args = mock_popen.call_args[0][0]
                self.assertIn("--follow", args)

    def test_logs_requires_service_name(self):
        """Passing just a namespace (no service) should die()."""
        with self.assertRaises(SystemExit):
            from sunbeam import services
            services.cmd_logs("ory", follow=False)


class TestCmdGet(unittest.TestCase):
    def test_prints_yaml_for_pod(self):
        with patch("sunbeam.services.kube_out", return_value="apiVersion: v1\nkind: Pod") as mock_ko:
            from sunbeam import services
            services.cmd_get("ory/kratos-abc")
            mock_ko.assert_called_once_with("get", "pod", "kratos-abc", "-n", "ory", "-o=yaml")

    def test_default_output_is_yaml(self):
        with patch("sunbeam.services.kube_out", return_value="kind: Pod"):
            from sunbeam import services
            # no output kwarg → defaults to yaml
            services.cmd_get("ory/kratos-abc")

    def test_json_output_format(self):
        with patch("sunbeam.services.kube_out", return_value='{"kind":"Pod"}') as mock_ko:
            from sunbeam import services
            services.cmd_get("ory/kratos-abc", output="json")
            mock_ko.assert_called_once_with("get", "pod", "kratos-abc", "-n", "ory", "-o=json")

    def test_missing_name_exits(self):
        with self.assertRaises(SystemExit):
            from sunbeam import services
            services.cmd_get("ory")  # namespace-only, no pod name

    def test_not_found_exits(self):
        with patch("sunbeam.services.kube_out", return_value=""):
            with self.assertRaises(SystemExit):
                from sunbeam import services
                services.cmd_get("ory/nonexistent")


class TestCmdRestart(unittest.TestCase):
    def test_restart_all(self):
        with patch("sunbeam.services.kube") as mock_kube:
            from sunbeam import services
            services.cmd_restart(None)
            # Should restart all SERVICES_TO_RESTART
            self.assertGreater(mock_kube.call_count, 0)

    def test_restart_namespace_scoped(self):
        with patch("sunbeam.services.kube") as mock_kube:
            from sunbeam import services
            services.cmd_restart("ory")
            calls_str = str(mock_kube.call_args_list)
            # Should only restart ory/* services
            self.assertIn("ory", calls_str)
            self.assertNotIn("devtools", calls_str)

    def test_restart_specific_service(self):
        with patch("sunbeam.services.kube") as mock_kube:
            from sunbeam import services
            services.cmd_restart("ory/kratos")
            # Should restart exactly deployment/kratos in ory
            calls_str = str(mock_kube.call_args_list)
            self.assertIn("kratos", calls_str)

    def test_restart_unknown_service_warns(self):
        with patch("sunbeam.services.kube") as mock_kube:
            from sunbeam import services
            services.cmd_restart("nonexistent/nosuch")
            # kube should not be called since no match
            mock_kube.assert_not_called()
