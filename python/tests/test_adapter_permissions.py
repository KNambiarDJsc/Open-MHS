"""The adapter SDK's cooperative permission checks (spec/permission-manifest.md)."""

from __future__ import annotations

import os
import tempfile
import unittest

import support  # noqa: F401  (puts python/ on sys.path)

from openmhs.adapter import AdapterError, Permissions


class PermissionTests(unittest.TestCase):
    def test_network_compares_scheme_host_and_port(self):
        p = Permissions(network=("opc.tcp://10.0.0.5:4840",))
        p.check_network("opc.tcp://10.0.0.5:4840/line1")
        for other in ("opc.tcp://10.0.0.5:4841", "opc.tcp://10.0.0.6:4840", "http://10.0.0.5:4840"):
            with self.subTest(other), self.assertRaises(AdapterError) as e:
                p.check_network(other)
            self.assertEqual(e.exception.kind, "not_sent")

    def test_files_must_be_declared_or_inside_a_declared_directory(self):
        with tempfile.TemporaryDirectory() as root:
            pki = os.path.join(root, "pki")
            os.mkdir(pki)
            p = Permissions(filesystem_read=(pki, os.path.join(root, "profile.json")))
            self.assertEqual(p.check_read(os.path.join(pki, "ca.der")), os.path.realpath(os.path.join(pki, "ca.der")))
            p.check_read(os.path.join(root, "profile.json"))
            for outside in (
                os.path.join(root, "secret.key"),
                os.path.join(pki, "..", "secret.key"),  # no escaping with ..
                os.path.join(root, "pki-other", "x"),  # a sibling sharing the prefix
            ):
                with self.subTest(outside), self.assertRaises(AdapterError) as e:
                    p.check_read(outside)
                self.assertIn("filesystem.read", e.exception.message)

    def test_nothing_declared_means_nothing_allowed(self):
        with self.assertRaises(AdapterError):
            Permissions().check_read("anything")
        with self.assertRaises(AdapterError):
            Permissions().check_serial("/dev/ttyUSB0")


if __name__ == "__main__":
    unittest.main()
