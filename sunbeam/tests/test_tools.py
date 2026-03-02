"""Tests for tools.py binary bundler."""
import hashlib
import stat
import unittest
from pathlib import Path
from unittest.mock import MagicMock, patch
import tempfile
import shutil


class TestSha256(unittest.TestCase):
    def test_computes_correct_hash(self):
        from sunbeam.tools import _sha256
        with tempfile.NamedTemporaryFile(delete=False) as f:
            f.write(b"hello world")
            f.flush()
            path = Path(f.name)
        try:
            expected = hashlib.sha256(b"hello world").hexdigest()
            self.assertEqual(_sha256(path), expected)
        finally:
            path.unlink()


class TestEnsureTool(unittest.TestCase):
    def setUp(self):
        self.tmpdir = tempfile.mkdtemp()
        self.cache_patcher = patch("sunbeam.tools.CACHE_DIR", Path(self.tmpdir))
        self.cache_patcher.start()

    def tearDown(self):
        self.cache_patcher.stop()
        shutil.rmtree(self.tmpdir, ignore_errors=True)

    def test_returns_cached_if_sha_matches(self):
        binary_data = b"#!/bin/sh\necho kubectl"
        dest = Path(self.tmpdir) / "kubectl"
        dest.write_bytes(binary_data)
        dest.chmod(dest.stat().st_mode | stat.S_IXUSR)
        expected_sha = hashlib.sha256(binary_data).hexdigest()
        tools_spec = {"kubectl": {"url": "http://x", "sha256": expected_sha}}
        with patch("sunbeam.tools.TOOLS", tools_spec):
            from sunbeam import tools
            result = tools.ensure_tool("kubectl")
            self.assertEqual(result, dest)

    def test_returns_cached_if_sha_empty(self):
        binary_data = b"#!/bin/sh\necho kubectl"
        dest = Path(self.tmpdir) / "kubectl"
        dest.write_bytes(binary_data)
        dest.chmod(dest.stat().st_mode | stat.S_IXUSR)
        tools_spec = {"kubectl": {"url": "http://x", "sha256": ""}}
        with patch("sunbeam.tools.TOOLS", tools_spec):
            from sunbeam import tools
            result = tools.ensure_tool("kubectl")
            self.assertEqual(result, dest)

    def test_downloads_on_cache_miss(self):
        binary_data = b"#!/bin/sh\necho kubectl"
        tools_spec = {"kubectl": {"url": "http://example.com/kubectl", "sha256": ""}}
        with patch("sunbeam.tools.TOOLS", tools_spec):
            with patch("urllib.request.urlopen") as mock_url:
                mock_resp = MagicMock()
                mock_resp.read.return_value = binary_data
                mock_resp.__enter__ = lambda s: s
                mock_resp.__exit__ = MagicMock(return_value=False)
                mock_url.return_value = mock_resp
                from sunbeam import tools
                result = tools.ensure_tool("kubectl")
                dest = Path(self.tmpdir) / "kubectl"
                self.assertTrue(dest.exists())
                self.assertEqual(dest.read_bytes(), binary_data)
                # Should be executable
                self.assertTrue(dest.stat().st_mode & stat.S_IXUSR)

    def test_raises_on_sha256_mismatch(self):
        binary_data = b"#!/bin/sh\necho fake"
        tools_spec = {"kubectl": {
            "url": "http://example.com/kubectl",
            "sha256": "a" * 64,  # wrong hash
        }}
        with patch("sunbeam.tools.TOOLS", tools_spec):
            with patch("urllib.request.urlopen") as mock_url:
                mock_resp = MagicMock()
                mock_resp.read.return_value = binary_data
                mock_resp.__enter__ = lambda s: s
                mock_resp.__exit__ = MagicMock(return_value=False)
                mock_url.return_value = mock_resp
                from sunbeam import tools
                with self.assertRaises(RuntimeError) as ctx:
                    tools.ensure_tool("kubectl")
                self.assertIn("SHA256 mismatch", str(ctx.exception))
                # Binary should be cleaned up
                self.assertFalse((Path(self.tmpdir) / "kubectl").exists())

    def test_redownloads_on_sha_mismatch_cached(self):
        """If cached binary has wrong hash, it's deleted and re-downloaded."""
        old_data = b"old binary"
        new_data = b"new binary"
        dest = Path(self.tmpdir) / "kubectl"
        dest.write_bytes(old_data)
        new_sha = hashlib.sha256(new_data).hexdigest()
        tools_spec = {"kubectl": {"url": "http://x/kubectl", "sha256": new_sha}}
        with patch("sunbeam.tools.TOOLS", tools_spec):
            with patch("urllib.request.urlopen") as mock_url:
                mock_resp = MagicMock()
                mock_resp.read.return_value = new_data
                mock_resp.__enter__ = lambda s: s
                mock_resp.__exit__ = MagicMock(return_value=False)
                mock_url.return_value = mock_resp
                from sunbeam import tools
                result = tools.ensure_tool("kubectl")
                self.assertEqual(dest.read_bytes(), new_data)

    def test_unknown_tool_raises_value_error(self):
        from sunbeam import tools
        with self.assertRaises(ValueError):
            tools.ensure_tool("notarealtool")


class TestRunTool(unittest.TestCase):
    def setUp(self):
        self.tmpdir = tempfile.mkdtemp()
        self.cache_patcher = patch("sunbeam.tools.CACHE_DIR", Path(self.tmpdir))
        self.cache_patcher.start()

    def tearDown(self):
        self.cache_patcher.stop()
        shutil.rmtree(self.tmpdir, ignore_errors=True)

    def test_kustomize_prepends_cache_dir_to_path(self):
        binary_data = b"#!/bin/sh"
        dest = Path(self.tmpdir) / "kustomize"
        dest.write_bytes(binary_data)
        dest.chmod(dest.stat().st_mode | stat.S_IXUSR)
        tools_spec = {"kustomize": {"url": "http://x", "sha256": ""}}
        with patch("sunbeam.tools.TOOLS", tools_spec):
            with patch("subprocess.run") as mock_run:
                mock_run.return_value = MagicMock(returncode=0)
                from sunbeam import tools
                tools.run_tool("kustomize", "build", ".")
                call_kwargs = mock_run.call_args[1]
                env = call_kwargs.get("env", {})
                self.assertTrue(env.get("PATH", "").startswith(str(self.tmpdir)))

    def test_non_kustomize_does_not_modify_path(self):
        binary_data = b"#!/bin/sh"
        dest = Path(self.tmpdir) / "kubectl"
        dest.write_bytes(binary_data)
        dest.chmod(dest.stat().st_mode | stat.S_IXUSR)
        tools_spec = {"kubectl": {"url": "http://x", "sha256": ""}}
        with patch("sunbeam.tools.TOOLS", tools_spec):
            with patch("subprocess.run") as mock_run:
                mock_run.return_value = MagicMock(returncode=0)
                from sunbeam import tools
                import os
                original_path = os.environ.get("PATH", "")
                tools.run_tool("kubectl", "get", "pods")
                call_kwargs = mock_run.call_args[1]
                env = call_kwargs.get("env", {})
                # PATH should not be modified (starts same as original)
                self.assertFalse(env.get("PATH", "").startswith(str(self.tmpdir)))
