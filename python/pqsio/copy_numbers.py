"""Explicit CN metadata. Native Rust owns parsing and atomic replacement."""
import ctypes as C
import json
import os
from types import MappingProxyType
from .inspection import _Callback


class CopyNumbers:
    def __init__(self, data):
        self.present = data['present']
        self.explicit = MappingProxyType(data['explicit'])
        self._contigs = frozenset(data['contigs'])

    def effective(self, contig):
        if contig not in self._contigs:
            raise ValueError(f'Unknown contig {contig!r}: not present in _contigsizes')
        return self.explicit.get(contig, 1)


class _Entry(C.Structure):
    _fields_ = [('contig', C.c_char_p), ('copy_number', C.c_uint64)]


def _function(name, args):
    from . import _library
    lib = _library()
    if not hasattr(lib, name):
        raise RuntimeError('Native pqsio library lacks copy-number capability; rebuild/update it. Legacy interfaces remain supported.')
    fn = getattr(lib, name)
    fn.argtypes, fn.restype = args, C.c_int32
    return fn


def read_copy_numbers(path):
    from . import _utf8, _check
    fn = _function('pqsio_read_copy_numbers_json', [C.c_char_p, _Callback, C.c_void_p])
    result, errors = [], []

    @_Callback
    def receive(data, size, _):
        try:
            result.append(json.loads(C.string_at(data, size)))
            return 0
        except BaseException as exc:
            errors.append(exc)
            return -1
    code = fn(_utf8(os.fspath(path)), receive, None)
    if errors:
        raise errors[0]
    _check(code)
    return CopyNumbers(result[0])


def _entries(values):
    from . import _utf8, _uint
    result = []
    for name, value in values.items():
        if not isinstance(name, str):
            raise ValueError('cn.info contig must be a string')
        if isinstance(value, bool) or not isinstance(value, int) or not 1 <= value <= 2**64-1:
            raise ValueError(f'cn.info contig {name!r}: CN must be an integer in 1..=18446744073709551615')
        result.append(_Entry(_utf8(name), _uint(value, 64)))
    return (_Entry * len(result))(*result)


def _modify(path, values, update):
    from . import _utf8, _check
    fn = _function('pqsio_set_copy_numbers', [C.c_char_p, C.POINTER(_Entry), C.c_size_t, C.c_uint32])
    entries = _entries(values)
    _check(fn(_utf8(os.fspath(path)), entries, len(entries), update))


def set_copy_numbers(path, values):
    """Replace all explicit declarations. An empty mapping writes an empty file."""
    _modify(path, values, 0)


def update_copy_numbers(path, values):
    """Replace passed keys, preserving others. An empty mapping does not write."""
    _modify(path, values, 1)


def _writer_set(writer, values):
    from . import _check
    writer._active()
    fn = _function('pqsio_writer_set_copy_numbers', [C.c_void_p, C.POINTER(_Entry), C.c_size_t])
    entries = _entries(values)
    _check(fn(writer._handle, entries, len(entries)))
