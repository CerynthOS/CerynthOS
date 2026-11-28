"""Reject dangerous signing targets and distinguish inventory from verification."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT=Path(__file__).resolve().parents[1]


class SigningBoundaryTests(unittest.TestCase):
    def fixture(self, root):
        paths={name:root/name for name in ['image','key','cert']}
        for name,path in paths.items():path.write_text('fixture-'+name)
        return paths

    def test_output_cannot_overwrite_private_key(self):
        with tempfile.TemporaryDirectory() as temporary:
            paths=self.fixture(Path(temporary));before=paths['key'].read_bytes()
            result=subprocess.run(['bash',str(ROOT/'scripts/sign-kernel.sh'),'--image',str(paths['image']),
                '--key',str(paths['key']),'--cert',str(paths['cert']),'--out',str(paths['key'])],capture_output=True)
            self.assertNotEqual(result.returncode,0)
            self.assertEqual(paths['key'].read_bytes(),before)

    def test_dangling_output_symlink_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary);paths=self.fixture(root);target=root/'output';target.symlink_to(root/'missing')
            result=subprocess.run(['bash',str(ROOT/'scripts/sign-kernel.sh'),'--image',str(paths['image']),
                '--key',str(paths['key']),'--cert',str(paths['cert']),'--out',str(target)],capture_output=True)
            self.assertNotEqual(result.returncode,0)
            self.assertFalse((root/'missing').exists())

    def test_verification_without_trusted_certificate_fails(self):
        with tempfile.TemporaryDirectory() as temporary:
            paths=self.fixture(Path(temporary))
            result=subprocess.run(['bash',str(ROOT/'scripts/verify-kernel.sh'),'--image',str(paths['image'])],capture_output=True,text=True)
            self.assertEqual(result.returncode,2)
            self.assertIn('trusted certificate',result.stderr)

    def test_missing_option_value_fails_cleanly(self):
        result=subprocess.run(['bash',str(ROOT/'scripts/sign-kernel.sh'),'--image'],capture_output=True)
        self.assertEqual(result.returncode,2)

    def test_listing_and_verification_cannot_be_combined(self):
        with tempfile.TemporaryDirectory() as temporary:
            paths=self.fixture(Path(temporary))
            result=subprocess.run(['bash',str(ROOT/'scripts/verify-kernel.sh'),'--image',str(paths['image']),'--cert',str(paths['cert']),'--list'],capture_output=True)
            self.assertEqual(result.returncode,2)


if __name__=='__main__':unittest.main()
