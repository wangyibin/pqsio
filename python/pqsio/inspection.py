"""Read-only inspection. Native Rust performs parsing, footer access and scans."""
import copy
import ctypes as C
import json
import os
from dataclasses import dataclass


@dataclass(frozen=True)
class Metadata:
    """Known and unknown metadata fields; dtype expressions become safe strings."""
    fields: dict

    def to_dict(self):
        return copy.deepcopy(self.fields)


@dataclass(frozen=True)
class Inspection:
    _data: dict

    @property
    def metadata(self):
        return Metadata(copy.deepcopy(self._data['metadata']))

    def to_dict(self):
        return copy.deepcopy(self._data)


@dataclass(frozen=True)
class ValidationReport:
    _data: dict

    @property
    def status(self):
        return self._data['status']

    @property
    def issues(self):
        return copy.deepcopy(self._data['issues'])

    def to_dict(self):
        return copy.deepcopy(self._data)


_Callback = C.CFUNCTYPE(C.c_int32, C.POINTER(C.c_uint8), C.c_size_t, C.c_void_p)


def _call(path, level=None, max_issues=100):
    from . import _library, _utf8, _check
    lib = _library()
    name = 'pqsio_inspect_json' if level is None else 'pqsio_validate_json'
    if not hasattr(lib, name):
        raise RuntimeError('Native pqsio library lacks inspection capability; rebuild/update it')
    fn = getattr(lib, name)
    fn.argtypes = ([C.c_char_p] + ([] if level is None else [C.c_uint32, C.c_size_t])
                   + [_Callback, C.c_void_p])
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

    args = [_utf8(os.fspath(path))]
    if level is not None:
        args.extend([level, max_issues])
    code = fn(*args, receive, None)
    if errors:
        raise errors[0]
    _check(code)
    return result[0]


def inspect(path):
    """Read metadata and Parquet footers only; does not certify record validity."""
    return Inspection(_call(path))


def validate(path, level='quick', *, max_issues=100):
    """Read-only quick/full check; inspect status: valid, invalid or incomplete.

    max_issues bounds stored examples; the full scan continues. Rows are
    one-based within original shards. Full scan memory includes two row groups.
    """
    if level not in ('quick', 'full'):
        raise ValueError("level must be 'quick' or 'full'")
    if isinstance(max_issues, bool) or not isinstance(max_issues, int) or not 0 < max_issues <= C.c_size_t(-1).value:
        raise ValueError('max_issues must be a positive size_t integer')
    return ValidationReport(_call(path, int(level == 'full'), max_issues))
