"""Optional q0/q1 row-group indexes; standard-library-only native bindings."""
import ctypes as C
import json
import os
from . import StreamingReader, _library, _uint, _utf8, _check
from .inspection import _Callback


class _Region(C.Structure):
    _fields_ = [('contig', C.c_char_p), ('start', C.c_uint64), ('end', C.c_uint64)]


def _require():
    lib = _library()
    for symbol in ('pqsio_build_index', 'pqsio_query_open', 'pqsio_query_stats_json',
                   'pqsio_stream_next_columns'):
        if not hasattr(lib, symbol):
            raise RuntimeError('Native pqsio library lacks region-query/index capability; '
                               'rebuild/update it. Legacy APIs remain supported.')
    lib.pqsio_build_index.argtypes = [C.c_char_p, C.c_uint32]
    lib.pqsio_build_index.restype = C.c_int32
    lib.pqsio_query_open.argtypes = [C.c_char_p, C.POINTER(_Region), C.c_size_t,
                                    C.c_uint8, C.c_uint32, C.c_uint32, C.c_uint64,
                                    C.c_uint32, C.c_uint32, C.POINTER(C.c_void_p)]
    lib.pqsio_query_open.restype = C.c_int32
    lib.pqsio_query_stats_json.argtypes = [C.c_void_p, _Callback, C.c_void_p]
    lib.pqsio_query_stats_json.restype = C.c_int32
    return lib


def build_index(path, *, quality="q0", rebuild=False):
    """Build q0 or q1 summaries without modifying Parquet. Rebuild is explicit.

    Dataset must remain immutable. Completed generations are retained for active
    readers. No summary cache; build decoding is bounded by the largest row group.
    """
    if not isinstance(rebuild, bool):
        raise ValueError('rebuild must be bool')
    if quality not in ("q0", "q1"):
        raise ValueError("quality must be 'q0' or 'q1'")
    lib = _require()
    if quality == "q0":
        _check(lib.pqsio_build_index(_utf8(os.fspath(path)), int(rebuild)))
    else:
        if not hasattr(lib, "pqsio_build_index_quality"):
            raise RuntimeError('Native pqsio library lacks q1 index capability; rebuild/update it')
        fn = lib.pqsio_build_index_quality
        fn.argtypes = [C.c_char_p, C.c_uint32, C.c_uint32]
        fn.restype = C.c_int32
        _check(fn(_utf8(os.fspath(path)), 1, int(rebuild)))


class QueryReader(StreamingReader):
    """Query 0-based half-open regions as a union, preserving order and source rows.

    index: auto/off/require. pairs_mode: either/both. concat filter_mode:
    matching_alignments/complete_reads. Complete reads and shard-local IDs always
    scan q0 sequentially; require still checks availability of a valid index.
    Otherwise min_mapq >= 1 selects q1. All index modes target the selected partition.
    stats['source_quality'] identifies the selected partition.
    q1 must be a complete, ordered MAPQ >= 1 view of q0.
    batch_rows controls output rows, not decoded bytes. stats.complete becomes
    true only after iteration reaches EOF; close preserves partial statistics.
    """
    def __init__(self, path, regions, min_mapq=0, *, index='auto', pairs_mode='either',
                 filter_mode=None, batch_rows=65536, boundary='rows'):
        self._handle = C.c_void_p()
        self._saved_stats = None
        self._lib = _require()
        min_mapq = _uint(min_mapq, 8)
        if min_mapq > 0 and filter_mode != 'complete_reads' and not hasattr(self._lib, 'pqsio_build_index_quality'):
            raise RuntimeError('Native pqsio library lacks q1 index capability; rebuild/update it')
        indices = {'auto': 0, 'off': 1, 'require': 2}
        pairs = {'either': 0, 'both': 1}
        boundaries = {'rows': 0, 'complete_reads': 1}
        filters = {None: 0, 'matching_alignments': 1, 'complete_reads': 2}
        if index not in indices or pairs_mode not in pairs or boundary not in boundaries or filter_mode not in filters:
            raise ValueError('Invalid query index, pairs_mode, boundary or filter_mode')
        batch_rows = _uint(batch_rows, 32)
        if not batch_rows:
            raise ValueError('batch_rows must be positive')
        values = []
        for name, start, end in regions:
            if not isinstance(name, str):
                raise ValueError('region contig must be a string')
            if isinstance(start, bool) or isinstance(end, bool):
                raise ValueError('region coordinates must be integers, not bool')
            values.append(_Region(_utf8(name), _uint(start, 64), _uint(end, 64)))
        native = (_Region * len(values))(*values)
        _check(self._lib.pqsio_query_open(_utf8(os.fspath(path)), native, len(native),
               _uint(min_mapq, 8), indices[index], pairs[pairs_mode], batch_rows,
               boundaries[boundary], filters[filter_mode], C.byref(self._handle)))
        self._metadata()

    @property
    def stats(self):
        if not self._handle.value:
            return dict(self._saved_stats) if self._saved_stats is not None else None
        result, errors = [], []

        @_Callback
        def receive(data, size, _):
            try:
                result.append(json.loads(C.string_at(data, size)))
                return 0
            except BaseException as exc:
                errors.append(exc)
                return -1

        code = self._lib.pqsio_query_stats_json(self._handle, receive, None)
        if errors:
            raise errors[0]
        _check(code)
        return result[0]

    def close(self):
        if self._handle.value:
            try:
                self._saved_stats = self.stats
            finally:
                super().close()
