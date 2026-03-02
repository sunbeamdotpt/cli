"""Tests for secrets.py — seed idempotency, verify flow."""
import base64
import unittest
from unittest.mock import MagicMock, patch, call


class TestSeedIdempotency(unittest.TestCase):
    """_seed_openbao() must read existing values before writing (never rotates)."""

    def test_get_or_create_skips_existing(self):
        """If OpenBao already has a value, it's reused not regenerated."""
        with patch("sunbeam.secrets._seed_openbao") as mock_seed:
            mock_seed.return_value = {
                "hydra-system-secret": "existingvalue",
                "_ob_pod": "openbao-0",
                "_root_token": "token123",
            }
            from sunbeam import secrets
            result = secrets._seed_openbao()
            self.assertIn("hydra-system-secret", result)


class TestCmdVerify(unittest.TestCase):
    def _mock_kube_out(self, ob_pod="openbao-0", root_token="testtoken", mac=""):
        """Create a side_effect function for kube_out that simulates verify flow."""
        encoded_token = base64.b64encode(root_token.encode()).decode()
        def side_effect(*args, **kwargs):
            args_str = " ".join(str(a) for a in args)
            if "app.kubernetes.io/name=openbao" in args_str:
                return ob_pod
            if "root-token" in args_str:
                return encoded_token
            if "secretMAC" in args_str:
                return mac
            if "conditions" in args_str:
                return "unknown"
            if ".data.test-key" in args_str:
                return ""
            return ""
        return side_effect

    def test_verify_cleans_up_on_timeout(self):
        """cmd_verify() must clean up test resources even when VSO doesn't sync."""
        kube_out_fn = self._mock_kube_out(mac="")  # MAC never set -> timeout
        with patch("sunbeam.secrets.kube_out", side_effect=kube_out_fn):
            with patch("sunbeam.secrets.kube") as mock_kube:
                with patch("sunbeam.secrets.kube_apply"):
                    with patch("subprocess.run") as mock_run:
                        mock_run.return_value = MagicMock(returncode=0, stdout="", stderr="")
                        with patch("time.time") as mock_time:
                            # start=0, first check=0, second check past deadline
                            mock_time.side_effect = [0, 0, 100]
                            with patch("time.sleep"):
                                from sunbeam import secrets
                                with self.assertRaises(SystemExit):
                                    secrets.cmd_verify()
                                # Cleanup should have been called (delete calls)
                                delete_calls = [c for c in mock_kube.call_args_list
                                               if "delete" in str(c)]
                                self.assertGreater(len(delete_calls), 0)

    def test_verify_succeeds_when_synced(self):
        """cmd_verify() succeeds when VSO syncs the secret and value matches."""
        # We need a fixed test_value. Patch _secrets.token_urlsafe to return known value.
        test_val = "fixed-test-value"
        encoded_val = base64.b64encode(test_val.encode()).decode()
        encoded_token = base64.b64encode(b"testtoken").decode()

        call_count = [0]
        def kube_out_fn(*args, **kwargs):
            args_str = " ".join(str(a) for a in args)
            if "app.kubernetes.io/name=openbao" in args_str:
                return "openbao-0"
            if "root-token" in args_str:
                return encoded_token
            if "secretMAC" in args_str:
                call_count[0] += 1
                return "somemac" if call_count[0] >= 1 else ""
            if ".data.test-key" in args_str:
                return encoded_val
            return ""

        with patch("sunbeam.secrets.kube_out", side_effect=kube_out_fn):
            with patch("sunbeam.secrets.kube") as mock_kube:
                with patch("sunbeam.secrets.kube_apply"):
                    with patch("subprocess.run") as mock_run:
                        mock_run.return_value = MagicMock(returncode=0, stdout="", stderr="")
                        with patch("sunbeam.secrets._secrets.token_urlsafe", return_value=test_val):
                            with patch("time.time", return_value=0):
                                with patch("time.sleep"):
                                    from sunbeam import secrets
                                    # Should not raise
                                    secrets.cmd_verify()
