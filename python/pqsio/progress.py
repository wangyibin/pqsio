"""CLI-only progress on stderr; native callbacks contain no record payloads."""
from contextlib import contextmanager
import ctypes as C
import sys
import time

_Callback = C.CFUNCTYPE(None, C.POINTER(C.c_uint8), C.c_size_t, C.c_uint64, C.c_uint64, C.c_void_p)


@contextmanager
def display(enabled, label):
    if enabled is False or (enabled is None and not sys.stderr.isatty()):
        yield
        return
    from rich.console import Console
    from rich.progress import Progress, SpinnerColumn, TextColumn, TimeElapsedColumn
    from . import _library
    console = Console(stderr=True)
    started = time.monotonic()
    current, since, stages = label, started, []
    setter = None
    with Progress(SpinnerColumn(), TextColumn('{task.description}'), TimeElapsedColumn(),
                  console=console, transient=True, refresh_per_second=4) as progress:
        task = progress.add_task(label, total=None)

        @_Callback
        def receive(data, size, completed, total, _):
            nonlocal current, since
            try:
                stage = C.string_at(data, size).decode('utf-8')
                now = time.monotonic()
                if stage != current:
                    stages.append((current, now-since))
                    current, since = stage, now
                suffix = f' · {completed:,}' if completed else ''
                if total:
                    suffix += f' / {total:,}'
                progress.update(task, description=stage+suffix)
            except Exception:
                # Progress is advisory; UI failures must not corrupt native I/O.
                pass

        try:
            lib = _library()
            if hasattr(lib, 'pqsio_set_progress_callback'):
                setter = lib.pqsio_set_progress_callback
                setter.argtypes = [_Callback, C.c_void_p]
                setter.restype = None
                setter(receive, None)
            yield
        except BrokenPipeError:
            raise
        except BaseException:
            console.print(f'Stopped during {current} after {time.monotonic()-started:.2f}s', markup=False)
            raise
        finally:
            if setter is not None:
                setter(_Callback(), None)
            stages.append((current, time.monotonic()-since))
    details = '; '.join(f'{stage}: {seconds:.2f}s' for stage, seconds in stages)
    console.print(f'Done in {time.monotonic()-started:.2f}s — {details}', markup=False)
