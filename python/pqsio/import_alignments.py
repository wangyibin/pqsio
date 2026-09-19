"""One-call binding to the Rust BAM/PAF importer; no Python record processing."""
import ctypes as C
import json
import os
from .inspection import _Callback


def import_alignments(input, output, mode, *, chunk_size, batch_rows, min_mapq,
                      min_order, max_order, threads, contigsizes, include_secondary,
                      samtools, tmpdir, pair_position):
    from . import _library, _utf8, _check
    if samtools is not None:
        raise ValueError('BAM imports now read directly in Rust; omit the obsolete samtools option')
    lib = _library()
    if not hasattr(lib, 'pqsio_import_json'):
        raise RuntimeError('Native pqsio library lacks BAM/PAF import capability; rebuild/update it')
    fn = lib.pqsio_import_json
    fn.argtypes = [C.c_char_p, C.c_char_p, C.c_char_p, C.c_size_t, C.c_size_t,
                   C.c_uint8, C.c_size_t, C.c_size_t, C.c_size_t, C.c_char_p,
                   C.c_uint32, C.c_char_p, C.c_uint32, _Callback, C.c_void_p]
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

    def path(value):
        return None if value is None else _utf8(os.fsdecode(value))

    code = fn(path(input), path(output), _utf8(mode), chunk_size, batch_rows,
              min_mapq, min_order, C.c_size_t(-1).value if max_order is None else max_order,
              threads, path(contigsizes), include_secondary, path(tmpdir),
              pair_position == 'five-prime', receive, None)
    if errors:
        raise errors[0]
    if code == -2:
        raise ValueError(lib.pqsio_last_error().decode('utf-8'))
    _check(code)
    return result[0]
