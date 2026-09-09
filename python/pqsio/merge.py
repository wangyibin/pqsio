"""Ordered native streaming merge; the Python boundary transfers only paths/JSON."""
import copy
import ctypes as C
import json
import os
from dataclasses import dataclass
from .inspection import _Callback


@dataclass(frozen=True)
class MergeResult:
    _data: dict

    def to_dict(self):
        return copy.deepcopy(self._data)


def merge(inputs, output, *, chunk_size=1_000_000, batch_rows=65_536, provenance=True):
    """Merge q0 in input order and regenerate q1, without deduplication.

    Inputs must remain unchanged. Concat reads receive new global IDs; pairs
    IDs need not be unique. batch_rows is a batch target, not an RSS limit:
    memory includes a decoded row group, Writer buffers and oversized reads.
    Application sidecars are not propagated; see result.sources via to_dict().
    """
    from . import _library, _utf8, _check
    if isinstance(inputs, (str, bytes, os.PathLike)):
        raise TypeError('inputs must be an iterable of dataset paths')
    paths = [_utf8(os.fspath(path)) for path in inputs]
    if not paths:
        raise ValueError('merge requires at least one input')
    for name, value, maximum in [('chunk_size', chunk_size, C.c_size_t(-1).value),
                                 ('batch_rows', batch_rows, min(2**32-1, C.c_size_t(-1).value))]:
        if isinstance(value, bool) or not isinstance(value, int) or not 0 < value <= maximum:
            raise ValueError(f'{name} must be an integer in 1..={maximum}')
    if not isinstance(provenance, bool):
        raise TypeError('provenance must be bool')
    lib = _library()
    if not hasattr(lib, 'pqsio_merge_json'):
        raise RuntimeError('Native pqsio library lacks merge capability; rebuild/update it')
    fn = lib.pqsio_merge_json
    fn.argtypes = [C.POINTER(C.c_char_p), C.c_size_t, C.c_char_p,
                   C.c_size_t, C.c_size_t, C.c_uint32, _Callback, C.c_void_p]
    fn.restype = C.c_int32
    result, errors = [], []

    @_Callback
    def receive(data, size, _):
        try:
            result.append(json.loads(C.string_at(data, size)))
            return 0
        except BaseException as exc:
            errors.append(exc)
            return -1

    code = fn((C.c_char_p * len(paths))(*paths), len(paths), _utf8(os.fspath(output)),
              chunk_size, batch_rows, int(provenance), receive, None)
    if errors:
        raise errors[0]
    _check(code)
    return MergeResult(result[0])
