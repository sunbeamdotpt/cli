"""Tests for kube.py — domain substitution, target parsing, kubectl wrappers."""
import unittest
from unittest.mock import MagicMock, patch


class TestParseTarget(unittest.TestCase):
    def setUp(self):
        from sunbeam.kube import parse_target
        self.parse = parse_target

    def test_none(self):
        self.assertEqual(self.parse(None), (None, None))

    def test_namespace_only(self):
        self.assertEqual(self.parse("ory"), ("ory", None))

    def test_namespace_and_name(self):
        self.assertEqual(self.parse("ory/kratos"), ("ory", "kratos"))

    def test_too_many_parts_raises(self):
        with self.assertRaises(ValueError):
            self.parse("too/many/parts")

    def test_empty_string(self):
        result = self.parse("")
        self.assertEqual(result, ("", None))


class TestDomainReplace(unittest.TestCase):
    def setUp(self):
        from sunbeam.kube import domain_replace
        self.replace = domain_replace

    def test_single_occurrence(self):
        result = self.replace("src.DOMAIN_SUFFIX/foo", "192.168.1.1.sslip.io")
        self.assertEqual(result, "src.192.168.1.1.sslip.io/foo")

    def test_multiple_occurrences(self):
        text = "DOMAIN_SUFFIX and DOMAIN_SUFFIX"
        result = self.replace(text, "x.sslip.io")
        self.assertEqual(result, "x.sslip.io and x.sslip.io")

    def test_no_occurrence(self):
        result = self.replace("no match here", "x.sslip.io")
        self.assertEqual(result, "no match here")


class TestKustomizeBuild(unittest.TestCase):
    def test_calls_run_tool_and_applies_domain_replace(self):
        from pathlib import Path
        mock_result = MagicMock()
        mock_result.stdout = "image: src.DOMAIN_SUFFIX/foo\nimage: src.DOMAIN_SUFFIX/bar"
        with patch("sunbeam.kube.run_tool", return_value=mock_result) as mock_rt:
            from sunbeam.kube import kustomize_build
            result = kustomize_build(Path("/some/overlay"), "192.168.1.1.sslip.io")
            mock_rt.assert_called_once()
            call_args = mock_rt.call_args[0]
            self.assertEqual(call_args[0], "kustomize")
            self.assertIn("build", call_args)
            self.assertIn("--enable-helm", call_args)
            self.assertIn("192.168.1.1.sslip.io", result)
            self.assertNotIn("DOMAIN_SUFFIX", result)

    def test_strips_null_annotations(self):
        from pathlib import Path
        mock_result = MagicMock()
        mock_result.stdout = "metadata:\n      annotations: null\n  name: test"
        with patch("sunbeam.kube.run_tool", return_value=mock_result):
            from sunbeam.kube import kustomize_build
            result = kustomize_build(Path("/overlay"), "x.sslip.io")
            self.assertNotIn("annotations: null", result)


class TestKubeWrappers(unittest.TestCase):
    def test_kube_passes_context(self):
        with patch("sunbeam.kube.run_tool") as mock_rt:
            mock_rt.return_value = MagicMock(returncode=0)
            from sunbeam.kube import kube
            kube("get", "pods")
            call_args = mock_rt.call_args[0]
            self.assertEqual(call_args[0], "kubectl")
            self.assertIn("--context=sunbeam", call_args)

    def test_kube_out_returns_stdout_on_success(self):
        with patch("sunbeam.kube.run_tool") as mock_rt:
            mock_rt.return_value = MagicMock(returncode=0, stdout="  output  ")
            from sunbeam.kube import kube_out
            result = kube_out("get", "pods")
            self.assertEqual(result, "output")

    def test_kube_out_returns_empty_on_failure(self):
        with patch("sunbeam.kube.run_tool") as mock_rt:
            mock_rt.return_value = MagicMock(returncode=1, stdout="error text")
            from sunbeam.kube import kube_out
            result = kube_out("get", "pods")
            self.assertEqual(result, "")

    def test_kube_ok_returns_true_on_zero(self):
        with patch("sunbeam.kube.run_tool") as mock_rt:
            mock_rt.return_value = MagicMock(returncode=0)
            from sunbeam.kube import kube_ok
            self.assertTrue(kube_ok("get", "ns", "default"))

    def test_kube_ok_returns_false_on_nonzero(self):
        with patch("sunbeam.kube.run_tool") as mock_rt:
            mock_rt.return_value = MagicMock(returncode=1)
            from sunbeam.kube import kube_ok
            self.assertFalse(kube_ok("get", "ns", "missing"))
