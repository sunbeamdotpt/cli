"""Tests for checks.py — service-level health probes."""
import json
import unittest
from unittest.mock import MagicMock, patch


class TestCheckGiteaVersion(unittest.TestCase):
    def test_ok_returns_version(self):
        body = json.dumps({"version": "1.21.0"}).encode()
        with patch("sunbeam.checks._http_get", return_value=(200, body)):
            from sunbeam import checks
            r = checks.check_gitea_version("testdomain", None)
        self.assertTrue(r.passed)
        self.assertIn("1.21.0", r.detail)

    def test_non_200_fails(self):
        with patch("sunbeam.checks._http_get", return_value=(502, b"")):
            from sunbeam import checks
            r = checks.check_gitea_version("testdomain", None)
        self.assertFalse(r.passed)
        self.assertIn("502", r.detail)

    def test_connection_error_fails(self):
        import urllib.error
        with patch("sunbeam.checks._http_get", side_effect=urllib.error.URLError("refused")):
            from sunbeam import checks
            r = checks.check_gitea_version("testdomain", None)
        self.assertFalse(r.passed)


class TestCheckGiteaAuth(unittest.TestCase):
    def _secret(self, key, val):
        def side_effect(ns, name, k):
            return val if k == key else "gitea_admin"
        return side_effect

    def test_ok_returns_login(self):
        body = json.dumps({"login": "gitea_admin"}).encode()
        with patch("sunbeam.checks._kube_secret",
                   side_effect=self._secret("admin-password", "hunter2")):
            with patch("sunbeam.checks._http_get", return_value=(200, body)):
                from sunbeam import checks
                r = checks.check_gitea_auth("testdomain", None)
        self.assertTrue(r.passed)
        self.assertIn("gitea_admin", r.detail)

    def test_missing_password_fails(self):
        with patch("sunbeam.checks._kube_secret", return_value=""):
            from sunbeam import checks
            r = checks.check_gitea_auth("testdomain", None)
        self.assertFalse(r.passed)
        self.assertIn("secret", r.detail)

    def test_non_200_fails(self):
        with patch("sunbeam.checks._kube_secret",
                   side_effect=self._secret("admin-password", "hunter2")):
            with patch("sunbeam.checks._http_get", return_value=(401, b"")):
                from sunbeam import checks
                r = checks.check_gitea_auth("testdomain", None)
        self.assertFalse(r.passed)


class TestCheckPostgres(unittest.TestCase):
    def test_ready_passes(self):
        with patch("sunbeam.checks.kube_out", side_effect=["1", "1"]):
            from sunbeam import checks
            r = checks.check_postgres("testdomain", None)
        self.assertTrue(r.passed)
        self.assertIn("1/1", r.detail)

    def test_not_ready_fails(self):
        with patch("sunbeam.checks.kube_out", side_effect=["0", "1"]):
            from sunbeam import checks
            r = checks.check_postgres("testdomain", None)
        self.assertFalse(r.passed)

    def test_cluster_not_found_fails(self):
        with patch("sunbeam.checks.kube_out", return_value=""):
            from sunbeam import checks
            r = checks.check_postgres("testdomain", None)
        self.assertFalse(r.passed)


class TestCheckValkey(unittest.TestCase):
    def test_pong_passes(self):
        with patch("sunbeam.checks.kube_out", return_value="valkey-abc"):
            with patch("sunbeam.checks.kube_exec", return_value=(0, "PONG")):
                from sunbeam import checks
                r = checks.check_valkey("testdomain", None)
        self.assertTrue(r.passed)

    def test_no_pod_fails(self):
        with patch("sunbeam.checks.kube_out", return_value=""):
            from sunbeam import checks
            r = checks.check_valkey("testdomain", None)
        self.assertFalse(r.passed)

    def test_no_pong_fails(self):
        with patch("sunbeam.checks.kube_out", return_value="valkey-abc"):
            with patch("sunbeam.checks.kube_exec", return_value=(1, "")):
                from sunbeam import checks
                r = checks.check_valkey("testdomain", None)
        self.assertFalse(r.passed)


class TestCheckOpenbao(unittest.TestCase):
    def test_unsealed_passes(self):
        out = json.dumps({"initialized": True, "sealed": False})
        with patch("sunbeam.checks.kube_exec", return_value=(0, out)):
            from sunbeam import checks
            r = checks.check_openbao("testdomain", None)
        self.assertTrue(r.passed)

    def test_sealed_fails(self):
        out = json.dumps({"initialized": True, "sealed": True})
        with patch("sunbeam.checks.kube_exec", return_value=(2, out)):
            from sunbeam import checks
            r = checks.check_openbao("testdomain", None)
        self.assertFalse(r.passed)

    def test_no_output_fails(self):
        with patch("sunbeam.checks.kube_exec", return_value=(1, "")):
            from sunbeam import checks
            r = checks.check_openbao("testdomain", None)
        self.assertFalse(r.passed)


class TestCheckSeaweedfs(unittest.TestCase):
    def test_200_passes(self):
        with patch("sunbeam.checks._http_get", return_value=(200, b"")):
            from sunbeam import checks
            r = checks.check_seaweedfs("testdomain", None)
        self.assertTrue(r.passed)

    def test_403_unauthenticated_passes(self):
        # S3 returns 403 for unauthenticated requests — that means it's up.
        with patch("sunbeam.checks._http_get", return_value=(403, b"")):
            from sunbeam import checks
            r = checks.check_seaweedfs("testdomain", None)
        self.assertTrue(r.passed)

    def test_502_fails(self):
        with patch("sunbeam.checks._http_get", return_value=(502, b"")):
            from sunbeam import checks
            r = checks.check_seaweedfs("testdomain", None)
        self.assertFalse(r.passed)

    def test_connection_error_fails(self):
        import urllib.error
        with patch("sunbeam.checks._http_get",
                   side_effect=urllib.error.URLError("refused")):
            from sunbeam import checks
            r = checks.check_seaweedfs("testdomain", None)
        self.assertFalse(r.passed)


class TestCheckKratos(unittest.TestCase):
    def test_200_passes(self):
        with patch("sunbeam.checks._http_get", return_value=(200, b"")):
            from sunbeam import checks
            r = checks.check_kratos("testdomain", None)
        self.assertTrue(r.passed)

    def test_503_fails(self):
        with patch("sunbeam.checks._http_get", return_value=(503, b"not ready")):
            from sunbeam import checks
            r = checks.check_kratos("testdomain", None)
        self.assertFalse(r.passed)
        self.assertIn("503", r.detail)


class TestCheckHydraOidc(unittest.TestCase):
    def test_200_with_issuer_passes(self):
        body = json.dumps({"issuer": "https://auth.testdomain/"}).encode()
        with patch("sunbeam.checks._http_get", return_value=(200, body)):
            from sunbeam import checks
            r = checks.check_hydra_oidc("testdomain", None)
        self.assertTrue(r.passed)
        self.assertIn("testdomain", r.detail)

    def test_502_fails(self):
        with patch("sunbeam.checks._http_get", return_value=(502, b"")):
            from sunbeam import checks
            r = checks.check_hydra_oidc("testdomain", None)
        self.assertFalse(r.passed)


class TestCheckPeople(unittest.TestCase):
    def test_200_passes(self):
        with patch("sunbeam.checks._http_get", return_value=(200, b"<html>")):
            from sunbeam import checks
            r = checks.check_people("testdomain", None)
        self.assertTrue(r.passed)

    def test_302_redirect_passes(self):
        with patch("sunbeam.checks._http_get", return_value=(302, b"")):
            from sunbeam import checks
            r = checks.check_people("testdomain", None)
        self.assertTrue(r.passed)
        self.assertIn("302", r.detail)

    def test_502_fails(self):
        with patch("sunbeam.checks._http_get", return_value=(502, b"")):
            from sunbeam import checks
            r = checks.check_people("testdomain", None)
        self.assertFalse(r.passed)
        self.assertIn("502", r.detail)


class TestCheckPeopleApi(unittest.TestCase):
    def test_200_passes(self):
        with patch("sunbeam.checks._http_get", return_value=(200, b"{}")):
            from sunbeam import checks
            r = checks.check_people_api("testdomain", None)
        self.assertTrue(r.passed)

    def test_401_auth_required_passes(self):
        with patch("sunbeam.checks._http_get", return_value=(401, b"")):
            from sunbeam import checks
            r = checks.check_people_api("testdomain", None)
        self.assertTrue(r.passed)

    def test_502_fails(self):
        with patch("sunbeam.checks._http_get", return_value=(502, b"")):
            from sunbeam import checks
            r = checks.check_people_api("testdomain", None)
        self.assertFalse(r.passed)


class TestCheckLivekit(unittest.TestCase):
    def test_responding_passes(self):
        with patch("sunbeam.checks.kube_out", return_value="livekit-server-abc"):
            with patch("sunbeam.checks.kube_exec", return_value=(0, "")):
                from sunbeam import checks
                r = checks.check_livekit("testdomain", None)
        self.assertTrue(r.passed)

    def test_no_pod_fails(self):
        with patch("sunbeam.checks.kube_out", return_value=""):
            from sunbeam import checks
            r = checks.check_livekit("testdomain", None)
        self.assertFalse(r.passed)

    def test_exec_fails(self):
        with patch("sunbeam.checks.kube_out", return_value="livekit-server-abc"):
            with patch("sunbeam.checks.kube_exec", return_value=(1, "")):
                from sunbeam import checks
                r = checks.check_livekit("testdomain", None)
        self.assertFalse(r.passed)


class TestCmdCheck(unittest.TestCase):
    def _run(self, target, mock_list):
        from sunbeam import checks
        result = checks.CheckResult("x", "ns", "svc", True, "ok")
        fns = [MagicMock(return_value=result) for _ in mock_list]
        patched = list(zip(fns, [ns for _, ns, _ in mock_list], [s for _, _, s in mock_list]))
        with patch("sunbeam.checks.get_domain", return_value="td"), \
             patch("sunbeam.checks._ssl_ctx", return_value=None), \
             patch("sunbeam.checks._opener", return_value=None), \
             patch.object(checks, "CHECKS", patched):
            checks.cmd_check(target)
        return fns

    def test_no_target_runs_all(self):
        mock_list = [("unused", "devtools", "gitea"), ("unused", "data", "postgres")]
        fns = self._run(None, mock_list)
        fns[0].assert_called_once_with("td", None)
        fns[1].assert_called_once_with("td", None)

    def test_ns_filter_skips_other_namespaces(self):
        mock_list = [("unused", "devtools", "gitea"), ("unused", "data", "postgres")]
        fns = self._run("devtools", mock_list)
        fns[0].assert_called_once()
        fns[1].assert_not_called()

    def test_svc_filter(self):
        mock_list = [("unused", "ory", "kratos"), ("unused", "ory", "hydra")]
        fns = self._run("ory/kratos", mock_list)
        fns[0].assert_called_once()
        fns[1].assert_not_called()

    def test_no_match_warns(self):
        from sunbeam import checks
        with patch("sunbeam.checks.get_domain", return_value="td"), \
             patch("sunbeam.checks._ssl_ctx", return_value=None), \
             patch("sunbeam.checks._opener", return_value=None), \
             patch.object(checks, "CHECKS", []), \
             patch("sunbeam.checks.warn") as mock_warn:
            checks.cmd_check("nonexistent")
        mock_warn.assert_called_once()
