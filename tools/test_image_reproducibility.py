"""Ensure reproducibility evidence rejects real content and source drift."""
from pathlib import Path
import tempfile
import unittest
from image_reproducibility import compare_files, tree_manifest


class DiskComparisonTests(unittest.TestCase):
    def test_identical_sparse_files(self):
        with tempfile.TemporaryDirectory() as temporary:
            a, b = [Path(temporary) / name for name in ['a', 'b']]
            for path in [a, b]:
                with path.open('wb') as stream:
                    stream.seek(2 * 1024**2); stream.write(b'payload')
            self.assertTrue(compare_files(a, b)['byte_identical'])

    def test_change_past_first_block_is_detected(self):
        with tempfile.TemporaryDirectory() as temporary:
            a, b = [Path(temporary) / name for name in ['a', 'b']]
            a.write_bytes(b'a' * (2 * 1024**2 + 7)); b.write_bytes(a.read_bytes())
            with b.open('r+b') as stream:
                stream.seek(2 * 1024**2 + 6); stream.write(b'b')
            result = compare_files(a, b)
            self.assertFalse(result['byte_identical'])
            self.assertEqual(result['mismatched_bytes'], 1)
            self.assertEqual(result['first_mismatching_blocks'][0]['first_difference'], 2 * 1024**2 + 6)

    def test_equal_prefix_with_extra_zero_tail_is_different(self):
        with tempfile.TemporaryDirectory() as temporary:
            a, b = [Path(temporary) / name for name in ['a', 'b']]
            a.write_bytes(b'prefix'); b.write_bytes(b'prefix\0')
            self.assertFalse(compare_files(a, b)['byte_identical'])

    def test_lock_rejects_source_content_and_mode_drift(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); source = root / 'payload'; source.write_text('before')
            baseline = tree_manifest(root)
            source.write_text('after')
            self.assertNotEqual(baseline, tree_manifest(root))
            baseline = tree_manifest(root)
            source.chmod(0o700)
            self.assertNotEqual(baseline, tree_manifest(root))

    def test_lock_includes_symlinks_and_empty_directories(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); (root / 'empty').mkdir(); (root / 'link').symlink_to('missing')
            result = {item['path']: item for item in tree_manifest(root)}
            self.assertEqual(result['link']['target'], 'missing')
            self.assertEqual(result['empty']['kind'], 'directory')


if __name__ == '__main__':
    unittest.main()
