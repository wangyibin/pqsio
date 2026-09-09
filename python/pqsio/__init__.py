"""PQS ABI v1 bindings. Set PQSIO_LIBRARY to the built shared library path."""
import ctypes as C
import ctypes.util
from dataclasses import dataclass, fields
import os

__version__ = "0.0.1"

@dataclass
class Pair:
    read_id: str
    chrom1: int
    pos1: int
    chrom2: int
    pos2: int
    strand1: str
    strand2: str
    mapq: int

@dataclass
class Alignment:
    read_idx: int
    read_length: int
    read_start: int
    read_end: int
    strand: str
    chrom: int
    start: int
    end: int
    mapping_quality: int
    identity: float
    filter_reason: str = "pass"

class _Contig(C.Structure):
    _fields_ = [("name", C.c_char_p), ("length", C.c_uint64)]

class _Pair(C.Structure):
    _fields_ = [("read_id", C.c_char_p), ("chrom1", C.c_uint32), ("pos1", C.c_uint64),
                ("chrom2", C.c_uint32), ("pos2", C.c_uint64),
                ("strand1", C.c_uint8), ("strand2", C.c_uint8), ("mapq", C.c_uint8)]

class _Alignment(C.Structure):
    _fields_ = [("read_idx", C.c_uint64), ("read_length", C.c_uint32),
                ("read_start", C.c_uint32), ("read_end", C.c_uint32),
                ("strand", C.c_uint8), ("chrom", C.c_uint32),
                ("start", C.c_uint64), ("end", C.c_uint64),
                ("mapping_quality", C.c_uint8), ("identity", C.c_float), ("filter_reason", C.c_char_p)]

_ContigsCB = C.CFUNCTYPE(C.c_int32, C.POINTER(_Contig), C.c_size_t, C.c_void_p)
_PairsCB = C.CFUNCTYPE(C.c_int32, C.POINTER(_Pair), C.c_size_t, C.c_void_p)
_ConcatCB = C.CFUNCTYPE(C.c_int32, C.POINTER(_Alignment), C.c_size_t, C.c_void_p)
_lib = None

def _library():
    global _lib
    if _lib is not None:
        return _lib
    path = os.environ.get("PQSIO_LIBRARY") or ctypes.util.find_library("pqsio")
    if not path:
        raise RuntimeError("Build pqsio and set PQSIO_LIBRARY to the absolute shared library path")
    lib = C.CDLL(path)
    signatures = {
        "abi_version": ([], C.c_uint32), "last_error": ([], C.c_char_p),
        "writer_open": ([C.c_char_p, C.c_uint32, C.POINTER(_Contig), C.c_size_t, C.c_size_t, C.POINTER(C.c_void_p)], C.c_int32),
        "write_pairs": ([C.c_void_p, C.POINTER(_Pair), C.c_size_t], C.c_int32),
        "write_read": ([C.c_void_p, C.POINTER(_Alignment), C.c_size_t], C.c_int32),
        "writer_finish": ([C.c_void_p], C.c_int32), "writer_destroy": ([C.c_void_p], C.c_int32),
        "reader_open": ([C.c_char_p, C.c_uint8, C.POINTER(C.c_void_p)], C.c_int32),
        "reader_kind": ([C.c_void_p], C.c_int32), "reader_destroy": ([C.c_void_p], C.c_int32),
        "reader_contigs": ([C.c_void_p, _ContigsCB, C.c_void_p], C.c_int32),
        "reader_next": ([C.c_void_p, _PairsCB, _ConcatCB, C.c_void_p], C.c_int32),
    }
    for name, (args, result) in signatures.items():
        fn = getattr(lib, "pqsio_" + name)
        fn.argtypes, fn.restype = args, result
    if lib.pqsio_abi_version() != 1:
        raise RuntimeError("Unsupported pqsio C ABI version")
    _lib = lib
    return lib

def _check(code):
    if code < 0:
        raise RuntimeError(_library().pqsio_last_error().decode("utf-8"))
    return code

def _uint(value, bits):
    if not isinstance(value, int) or not 0 <= value < 2 ** bits:
        raise ValueError(f"Expected unsigned {bits}-bit integer, got {value!r}")
    return value

def _utf8(value):
    data = str(value).encode("utf-8")
    if b"\0" in data:
        raise ValueError("Strings must not contain NUL")
    return data

def _encode(row, cls):
    values = []
    for name, dtype in cls._fields_:
        value = getattr(row, name)
        if dtype == C.c_char_p:
            value = _utf8(value)
        elif name.startswith("strand"):
            if value not in ("+", "-"):
                raise ValueError("Strand must be + or -")
            value = ord(value)
        elif dtype != C.c_float:
            value = _uint(value, C.sizeof(dtype) * 8)
        values.append(value)
    return cls(*values)

def _decode(row, cls):
    values = {}
    for f in fields(cls):
        value = getattr(row, f.name)
        if isinstance(value, bytes):
            value = value.decode("utf-8")
        elif f.name.startswith("strand"):
            value = chr(value)
        values[f.name] = value
    return cls(**values)

class _Writer:
    def __init__(self, path, contigs, chunk_size=1_000_000):
        self._handle = C.c_void_p()
        self._lib = _library()
        entries = list(contigs.items()) if hasattr(contigs, "items") else list(contigs)
        cs = (_Contig * len(entries))(*[_Contig(_utf8(n), _uint(s, 64)) for n, s in entries])
        _check(self._lib.pqsio_writer_open(_utf8(os.fspath(path)), self._kind, cs, len(cs), _uint(chunk_size, C.sizeof(C.c_size_t)*8), C.byref(self._handle)))
    def _active(self):
        if not self._handle.value:
            raise RuntimeError("Writer is closed")
    def finish(self):
        self._active()
        try:
            _check(self._lib.pqsio_writer_finish(self._handle))
        finally:
            self.close()
    def close(self):
        if self._handle.value:
            self._lib.pqsio_writer_destroy(self._handle)
            self._handle = C.c_void_p()
    def __enter__(self):
        self._active()
        return self
    def __exit__(self, exc_type, exc, tb):
        if exc_type is None and self._handle.value:
            self.finish()
        else:
            self.close()
    def __del__(self):
        if getattr(self, "_handle", None) and self._handle.value:
            self.close()

class PairsWriter(_Writer):
    _kind = 0
    def write_batch(self, rows):
        self._active()
        rows = list(rows)
        batch = (_Pair * len(rows))(*[_encode(r, _Pair) for r in rows])
        _check(self._lib.pqsio_write_pairs(self._handle, batch, len(batch)))

class ConcatWriter(_Writer):
    _kind = 1
    def write_read(self, rows):
        """Submit exactly one complete read; read IDs must strictly increase."""
        self._active()
        rows = list(rows)
        batch = (_Alignment * len(rows))(*[_encode(r, _Alignment) for r in rows])
        _check(self._lib.pqsio_write_read(self._handle, batch, len(batch)))

class Reader:
    def __init__(self, path, min_mapq=0):
        self._handle = C.c_void_p()
        self._lib = _library()
        _check(self._lib.pqsio_reader_open(_utf8(os.fspath(path)), _uint(min_mapq, 8), C.byref(self._handle)))
        try:
            self.kind = ("pairs", "concat")[_check(self._lib.pqsio_reader_kind(self._handle))]
            self.contigs = []
            errors = []
            @_ContigsCB
            def callback(rows, n, _):
                try:
                    self.contigs = [(rows[i].name.decode("utf-8"), rows[i].length) for i in range(n)]
                    return 0
                except BaseException as exc:
                    errors.append(exc)
                    return -1
            code = self._lib.pqsio_reader_contigs(self._handle, callback, None)
            if errors:
                raise errors[0]
            _check(code)
        except BaseException:
            self.close()
            raise
    def iter_batches(self):
        while True:
            if not self._handle.value:
                raise RuntimeError("Reader is closed")
            batch, errors = [], []
            def receive(rows, n, cls):
                try:
                    batch.extend(_decode(rows[i], cls) for i in range(n))
                    return 0
                except BaseException as exc:
                    errors.append(exc)
                    return -1
            pairs = _PairsCB(lambda rows, n, _: receive(rows, n, Pair))
            concat = _ConcatCB(lambda rows, n, _: receive(rows, n, Alignment))
            code = self._lib.pqsio_reader_next(self._handle, pairs, concat, None)
            if errors:
                raise errors[0]
            if _check(code) == 0:
                return
            yield batch
    def iter_reads(self):
        """Group consecutive concat alignments, including across shard boundaries.

        With min_mapq > 0, yields only retained alignments of each read.
        """
        if self.kind != "concat":
            raise ValueError("iter_reads requires a concat dataset")
        pending = []
        for batch in self.iter_batches():
            for row in batch:
                if pending and row.read_idx != pending[-1].read_idx:
                    yield pending
                    pending = []
                pending.append(row)
        if pending:
            yield pending
    def close(self):
        if self._handle.value:
            self._lib.pqsio_reader_destroy(self._handle)
            self._handle = C.c_void_p()
    def __enter__(self):
        return self
    def __exit__(self, *args):
        self.close()
    def __del__(self):
        if getattr(self, "_handle", None) and self._handle.value:
            self.close()

class PairsReader(Reader):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        if self.kind != "pairs":
            self.close()
            raise ValueError("Expected pairs PQS")

class ConcatReader(Reader):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        if self.kind != "concat":
            self.close()
            raise ValueError("Expected concat PQS")
