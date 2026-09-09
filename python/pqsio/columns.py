"""Typed column batches; numeric buffers use native-endian fixed-width types.

Submission accepts one-dimensional contiguous buffers (array or memoryview).
Writable buffers are borrowed for the synchronous call; readonly inputs get
one bulk copy. Never mutate/resize submitted buffers concurrently. Read results
are independent arrays: their memoryviews remain valid after batch/reader close.
"""
from array import array
import ctypes as C
from dataclasses import dataclass

_TYPES = {"B": C.c_uint8, "I": C.c_uint32, "Q": C.c_uint64, "f": C.c_float}


def pack_strings(values):
    """Pack str values into UInt64 offsets and UTF-8 bytes (empty strings allowed)."""
    offsets, data = array("Q", [0]), array("B")
    for value in values:
        data.frombytes(value.encode("utf-8"))
        offsets.append(len(data))
    return offsets, data


def _span_type(code):
    class Span(C.Structure):
        _fields_ = [("data", C.POINTER(_TYPES[code])), ("len", C.c_size_t)]
    return Span


_SPANS = {code: _span_type(code) for code in _TYPES}

@dataclass
class PairColumns:
    """Pair buffers; offsets are UInt64 and strands are ASCII bytes."""
    read_id_offsets: object
    read_id_bytes: object
    chrom1: object
    pos1: object
    chrom2: object
    pos2: object
    strand1: object
    strand2: object
    mapq: object
    _schema = [('read_id_offsets', 'Q'), ('read_id_bytes', 'B'), ('chrom1', 'I'), ('pos1', 'Q'), ('chrom2', 'I'), ('pos2', 'Q'), ('strand1', 'B'), ('strand2', 'B'), ('mapq', 'B')]

    def __len__(self):
        return len(self.pos1)

class _PairColumns(C.Structure):
    _fields_ = [(name, _SPANS[code]) for name, code in PairColumns._schema]

@dataclass
class ConcatColumns:
    """Concat buffers; offsets are UInt64 and strands are ASCII bytes."""
    read_offsets: object
    read_idx: object
    read_length: object
    read_start: object
    read_end: object
    strand: object
    chrom: object
    start: object
    end: object
    mapping_quality: object
    identity: object
    filter_reason_offsets: object
    filter_reason_bytes: object
    _schema = [('read_offsets', 'Q'), ('read_idx', 'Q'), ('read_length', 'I'), ('read_start', 'I'), ('read_end', 'I'), ('strand', 'B'), ('chrom', 'I'), ('start', 'Q'), ('end', 'Q'), ('mapping_quality', 'B'), ('identity', 'f'), ('filter_reason_offsets', 'Q'), ('filter_reason_bytes', 'B')]

    def __len__(self):
        return len(self.read_idx)

class _ConcatColumns(C.Structure):
    _fields_ = [(name, _SPANS[code]) for name, code in ConcatColumns._schema]


def _require(lib):
    if not hasattr(lib, "pqsio_columnar_version"):
        raise RuntimeError("Columnar API requires a native library with pqsio_columnar_version >= 1; row APIs remain available")
    lib.pqsio_columnar_version.argtypes = []
    lib.pqsio_columnar_version.restype = C.c_uint32
    if lib.pqsio_columnar_version() != 1:
        raise RuntimeError("Unsupported pqsio columnar extension version")
    signatures = {
        "write_pairs_columns": [C.c_void_p, C.POINTER(_PairColumns)],
        "write_concat_columns": [C.c_void_p, C.POINTER(_ConcatColumns)],
        "reader_next_columns": [C.c_void_p, C.POINTER(C.c_void_p)],
        "column_batch_pairs": [C.c_void_p, C.POINTER(_PairColumns)],
        "column_batch_concat": [C.c_void_p, C.POINTER(_ConcatColumns)],
        "column_batch_destroy": [C.c_void_p],
    }
    for name, args in signatures.items():
        fn = getattr(lib, "pqsio_" + name)
        fn.argtypes, fn.restype = args, C.c_int32


def _borrow(batch, cls):
    spans, keepalive = [], []
    for name, code in batch._schema:
        view = memoryview(getattr(batch, name))
        typ = _TYPES[code]
        if (view.ndim != 1 or not view.c_contiguous or view.format not in (code, "@" + code)
                or view.itemsize != C.sizeof(typ)):
            raise ValueError(f"{name} requires a contiguous native {code} buffer of width {C.sizeof(typ)}")
        storage = (typ * len(view))
        storage = storage.from_buffer_copy(view) if view.readonly else storage.from_buffer(view)
        if len(view) and C.addressof(storage) % C.alignment(typ):
            raise ValueError(f"{name} is not naturally aligned")
        spans.append(_SPANS[code](storage, len(view)))
        keepalive.extend((view, storage))
    return cls(*spans), keepalive


def write_columns(writer, batch):
    from . import _check
    writer._active()
    _require(writer._lib)
    expected, native, name = ((PairColumns, _PairColumns, "pairs") if writer._kind == 0
                              else (ConcatColumns, _ConcatColumns, "concat"))
    if not isinstance(batch, expected):
        raise TypeError(f"Expected {expected.__name__}")
    columns, keepalive = _borrow(batch, native)
    _check(getattr(writer._lib, "pqsio_write_" + name + "_columns")(writer._handle, C.byref(columns)))


def iter_columns(reader):
    from . import _check
    _require(reader._lib)
    cls, native, name = ((PairColumns, _PairColumns, "pairs") if reader.kind == "pairs"
                         else (ConcatColumns, _ConcatColumns, "concat"))
    while True:
        if not reader._handle.value:
            raise RuntimeError("Reader is closed")
        handle = C.c_void_p()
        if _check(reader._lib.pqsio_reader_next_columns(reader._handle, C.byref(handle))) == 0:
            return
        try:
            view = native()
            _check(getattr(reader._lib, "pqsio_column_batch_" + name)(handle, C.byref(view)))
            arrays = {}
            for field, code in cls._schema:
                span = getattr(view, field)
                data = array(code, [0]) * span.len
                if data.itemsize != C.sizeof(_TYPES[code]):
                    raise RuntimeError("Platform array width does not match the columnar ABI")
                if span.len:
                    # One native -> Python bulk copy per column, no transient bytes object.
                    C.memmove(data.buffer_info()[0], span.data, span.len * data.itemsize)
                arrays[field] = data
            batch = cls(**arrays)
        finally:
            reader._lib.pqsio_column_batch_destroy(handle)
        yield batch
