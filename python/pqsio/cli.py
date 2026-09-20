"""Optional compatibility launcher; all CLI parsing/execution is in Rust."""
import os
from pathlib import Path
import shutil
import sys


def main(argv=None):
    configured = os.environ.get('PQSIO_BINARY')
    candidates = [configured] if configured else [
        Path(__file__).resolve().parents[2] / 'target/dev-release/pqsio',
        shutil.which('pqsio'),
    ]
    for candidate in candidates:
        if candidate and Path(candidate).is_file() and os.access(candidate, os.X_OK):
            # Avoid recursion through an obsolete pip-installed Python launcher.
            with open(candidate, 'rb') as stream:
                if stream.read(4) != b'\x7fELF':
                    continue
            os.execv(str(candidate), [str(candidate), *(sys.argv[1:] if argv is None else argv)])
    print('pqsio: native executable not found; run pixi run build and add '
          'target/dev-release to PATH, or set PQSIO_BINARY', file=sys.stderr)
    return 1
