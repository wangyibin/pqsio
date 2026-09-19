"""Allow ``python -m pqsio`` alongside the installed ``pqsio`` command."""
import sys

from .cli import main


if __name__ == '__main__':
    sys.exit(main())
