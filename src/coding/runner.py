"""Trusted container launcher; the host never imports or executes candidate files."""
import json
import pathlib
import sys
import unittest
import traceback

payload = json.load(sys.stdin)
root = pathlib.Path('/workspace')
for item in payload['files']:
    relative = pathlib.PurePosixPath(item['path'])
    if relative.is_absolute() or '..' in relative.parts:
        raise ValueError('invalid artifact path')
    target = root / relative
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(item['content'], encoding='utf-8')
(root / 'test_run.py').write_text(payload['tests'], encoding='utf-8')
sys.path.insert(0, str(root))
try:
    suite = unittest.defaultTestLoader.discover(str(root), pattern='test_run.py')
except BaseException:
    # An import-time exit(0) is not a successful test run.
    traceback.print_exc()
    sys.exit(3)
count = suite.countTestCases()
if count == 0:
    print('No executable unittest cases were collected.', file=sys.stderr)
    sys.exit(2)
result = unittest.TextTestRunner(verbosity=2).run(suite)
sys.exit(0 if result.wasSuccessful() else 1)
