import shutil
import stat
import sys
import tempfile
import unittest
from pathlib import Path

from qeval import sandbox
from qeval.adapters import quadratique


@unittest.skipIf(sys.platform == "win32", "QUA-40: the harness runs commands in POSIX process groups")
class QuadratiqueHome(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix="qeval-test-"))
        self.addCleanup(shutil.rmtree, self.root)

    def made(self, root: Path, binaries: dict[str, Path] | None = None) -> sandbox.Sandbox:
        made = sandbox.Sandbox(root, "quadratique", binaries or {}).create()
        self.addCleanup(shutil.rmtree, made.qhome, ignore_errors=True)
        return made

    def test_a_deep_run_folder_gets_a_private_data_folder_whose_socket_path_is_short(self):
        deep = self.root.joinpath(*["a-run-folder-with-a-long-name"] * 6)

        made = self.made(deep)

        self.assertLessEqual(len(str(made.qhome / "server.sock").encode()), sandbox.SOCKET_PATH_LIMIT)
        self.assertEqual(stat.S_IMODE(made.qhome.stat().st_mode), 0o700)
        self.assertEqual(made.tool_env()["QUADRATIQUE_HOME"], str(made.qhome))

    def test_each_sandbox_gets_a_private_unguessable_data_folder_that_its_removal_keeps_in_the_run_and_deletes(self):
        first = self.made(self.root / "run")
        (first.qhome / "workspace.db").write_text("the store")
        other = self.made(self.root / "run")
        self.assertNotEqual(first.qhome, other.qhome)
        other.remove()

        first.remove()
        again = self.made(self.root / "run")

        self.assertFalse(first.qhome.exists(), "the removed sandbox left its data folder")
        self.assertEqual((first.root / "qhome" / "workspace.db").read_text(), "the store")
        self.assertEqual((again.qhome / "workspace.db").read_text(), "the store")
        self.assertNotEqual(again.qhome, first.qhome)

    def test_a_server_that_does_not_stop_fails_the_run(self):
        fake = self.root / "fake-quadratique"
        fake.write_text("#!/bin/sh\necho 'the server did not stop' >&2\nexit 69\n")
        fake.chmod(0o755)
        made = self.made(self.root / "run", {"quadratique": fake})

        with self.assertRaises(RuntimeError) as raised:
            quadratique.stop(made, self.root)

        self.assertIn("the server did not stop", str(raised.exception))


if __name__ == "__main__":
    unittest.main()
