"""Tests for manifests.py — primarily _filter_by_namespace."""
import unittest

from sunbeam.manifests import _filter_by_namespace


MULTI_DOC = """\
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: meet-config
  namespace: lasuite
data:
  FOO: bar
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: meet-backend
  namespace: lasuite
spec:
  replicas: 1
---
apiVersion: v1
kind: Namespace
metadata:
  name: lasuite
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: pingora-config
  namespace: ingress
data:
  config.toml: |
    hello
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: pingora
  namespace: ingress
spec:
  replicas: 1
"""


class TestFilterByNamespace(unittest.TestCase):

    def test_keeps_matching_namespace(self):
        result = _filter_by_namespace(MULTI_DOC, "lasuite")
        self.assertIn("name: meet-config", result)
        self.assertIn("name: meet-backend", result)

    def test_excludes_other_namespaces(self):
        result = _filter_by_namespace(MULTI_DOC, "lasuite")
        self.assertNotIn("namespace: ingress", result)
        self.assertNotIn("name: pingora-config", result)
        self.assertNotIn("name: pingora", result)

    def test_includes_namespace_resource_itself(self):
        result = _filter_by_namespace(MULTI_DOC, "lasuite")
        self.assertIn("kind: Namespace", result)

    def test_ingress_filter(self):
        result = _filter_by_namespace(MULTI_DOC, "ingress")
        self.assertIn("name: pingora-config", result)
        self.assertIn("name: pingora", result)
        self.assertNotIn("namespace: lasuite", result)

    def test_unknown_namespace_returns_empty(self):
        result = _filter_by_namespace(MULTI_DOC, "nonexistent")
        self.assertEqual(result.strip(), "")

    def test_empty_input_returns_empty(self):
        result = _filter_by_namespace("", "lasuite")
        self.assertEqual(result.strip(), "")

    def test_result_is_valid_multidoc_yaml(self):
        # Each non-empty doc in the result should start with '---'
        result = _filter_by_namespace(MULTI_DOC, "lasuite")
        self.assertTrue(result.startswith("---"))

    def test_does_not_include_namespace_resource_for_wrong_ns(self):
        # The lasuite Namespace CR should NOT appear in an ingress-filtered result
        result = _filter_by_namespace(MULTI_DOC, "ingress")
        # There's no ingress Namespace CR in the fixture, so kind: Namespace should be absent
        self.assertNotIn("kind: Namespace", result)

    def test_single_doc_matching(self):
        doc = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: x\n  namespace: ory\n"
        result = _filter_by_namespace(doc, "ory")
        self.assertIn("name: x", result)

    def test_single_doc_not_matching(self):
        doc = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: x\n  namespace: ory\n"
        result = _filter_by_namespace(doc, "lasuite")
        self.assertEqual(result.strip(), "")
