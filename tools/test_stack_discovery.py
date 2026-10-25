import importlib.util
from pathlib import Path
import tempfile
import unittest

spec=importlib.util.spec_from_file_location('sampler',Path(__file__).with_name('sample-scheduler-stacks.py'))
sampler=importlib.util.module_from_spec(spec)
spec.loader.exec_module(sampler)


class DiscoveryTests(unittest.TestCase):
    def test_cold_start_child_is_visible_without_heartbeat_and_reporter_is_excluded(self):
        with tempfile.TemporaryDirectory() as temp:
            root=Path(temp)
            for pid,parent,exe in [(10,1,'cerynthd'),(20,10,'cerynth-scx'),(21,20,'cerynth-scx'),(22,10,'unrelated')]:
                proc=root/str(pid)
                proc.mkdir()
                (proc/'exe').symlink_to('/usr/lib/cerynth/'+exe)
                (proc/'stat').write_text(f'{pid} ({exe}) S {parent} '+'0 '*20)
            (root/'10/task/10').mkdir(parents=True)
            (root/'10/task/10/children').write_text('20 21 22')
            # No scheduler.json or scheduler-stats.json exists in this fixture.
            self.assertEqual([r[0] for r in sampler.scheduler_children(10,root)],[20])
            (root/'10/exe').unlink()
            (root/'10/exe').symlink_to('/usr/bin/unrelated')
            self.assertEqual(sampler.scheduler_children(10,root),[])


if __name__=='__main__':unittest.main()
