"""Fast isolated tests for the remote researcher session's boundary and inputs."""
import importlib.util
import io
import json
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch, MagicMock

PATH = Path(__file__).with_name("remote_researcher.py")
spec = importlib.util.spec_from_file_location("remote_researcher", PATH)
module = importlib.util.module_from_spec(spec)
assert spec.loader
spec.loader.exec_module(module)


class RemoteSessionTests(unittest.TestCase):
    def test_session_name_validation(self):
        self.assertEqual(module.principal_for("college-b_01"), "agent:remote-researcher:college-b_01")
        for value in ("", "../escape", "a b", "é", "!" * 70):
            with self.assertRaises(ValueError):
                module.principal_for(value)

    @patch.object(module.Client, "from_home")
    def test_issue_scopes_are_read_only(self, from_home):
        op = from_home.return_value
        op.create_token.return_value = MagicMock(id="tid", token="secret")
        out = io.StringIO()
        with redirect_stdout(out):
            module.issue("home", "uni-test", 12)
        scopes = op.create_token.call_args.kwargs["scopes"]
        self.assertEqual(len(scopes), 3)
        self.assertEqual({s["capability"] for s in scopes}, set(module.CAPABILITIES))
        self.assertEqual({tuple(s["operations"]) for s in scopes}, {("read",)})
        self.assertEqual(op.create_token.call_args.kwargs["ttl_ms"], 720000)
        self.assertNotIn("any_scope", op.create_token.call_args.kwargs)
        self.assertEqual(op.add_grant.call_count, 3)
        self.assertIn("token_id", out.getvalue())
        self.assertNotIn("operator", out.getvalue().split("SECRET TOKEN")[0])

    def test_ttl_bounds(self):
        for minutes in (0, 61):
            with self.assertRaises(ValueError):
                module.issue("not-queried", "abc", minutes)

    @patch.object(module.Client, "from_home")
    def test_revocation(self, from_home):
        with redirect_stdout(io.StringIO()):
            module.revoke("home", "token-1")
        from_home.return_value.revoke_token.assert_called_once_with("token-1")

    def test_observe_requires_secret_and_loopback(self):
        with patch.dict("os.environ", {}, clear=True):
            with self.assertRaises(ValueError):
                module.observe("http://127.0.0.1:1234")
        with patch.dict("os.environ", {"OPENMHS_REMOTE_TOKEN": "secret"}):
            for url in ("https://example.org", "http://1.2.3.4:1234"):
                with self.assertRaises(ValueError):
                    module.observe(url)


if __name__ == "__main__":
    unittest.main()
