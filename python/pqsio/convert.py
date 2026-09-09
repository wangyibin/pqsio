"""Native streaming conversion between PQS formats."""
import copy
import ctypes as C
import json
import os
from dataclasses import dataclass
from .inspection import _Callback


@dataclass(frozen=True)
class ConvertResult:
    _data: dict

    def to_dict(self):
        return copy.deepcopy(self._data)


def convert(input, output, mode='concat2pairs', *, chunk_size=1_000_000,
            batch_rows=65_536, min_mapq=0, min_order=2, max_order=None, threads=1):
    """Expand concat PQS reads into pairs PQS; max_order is exclusive.

    Order filtering follows MAPQ filtering. Positions are 1-based midpoints.
    threads > 1 enables both conversion workers and a bounded encoding pool.
    Existing output is rejected. See README for IDs and memory limits.
    """
    from . import _library, _utf8, _check
    if mode != 'concat2pairs':
        raise ValueError('mode must be concat2pairs')
    size_max = C.c_size_t(-1).value
    if max_order is None:
        max_order = size_max
    for name, value, minimum, maximum in [
        ('threads', threads, 1, size_max),
        ('chunk_size', chunk_size, 1, size_max),
        ('batch_rows', batch_rows, 1, min(2**32-1, size_max)),
        ('min_mapq', min_mapq, 0, 255),
        ('min_order', min_order, 2, size_max),
        ('max_order', max_order, 3, size_max),
    ]:
        if isinstance(value, bool) or not isinstance(value, int) or not minimum <= value <= maximum:
            raise ValueError(f'{name} must be an integer in {minimum}..={maximum}')
    if max_order <= min_order:
        raise ValueError('max_order must exceed min_order (exclusive upper bound)')
    lib = _library()
    symbol = 'pqsio_convert_json' if threads == 1 else 'pqsio_convert_parallel_json'
    if not hasattr(lib, symbol):
        raise RuntimeError('Native pqsio library lacks convert capability; rebuild/update it')
    fn = getattr(lib, symbol)
    fn.argtypes = [C.c_char_p, C.c_char_p, C.c_char_p, C.c_size_t, C.c_size_t,
                   C.c_uint8, C.c_size_t, C.c_size_t] + (
                       [C.c_size_t] if threads != 1 else []) + [_Callback, C.c_void_p]
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

    code = fn(_utf8(os.fspath(input)), _utf8(os.fspath(output)), _utf8(mode),
              chunk_size, batch_rows, min_mapq, min_order, max_order,
              *([threads] if threads != 1 else []), receive, None)
    if errors:
        raise errors[0]
    _check(code)
    return ConvertResult(result[0])
