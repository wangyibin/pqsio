"""PQS ABI v1 bindings. Set PQSIO_LIBRARY to the built shared library path."""
import ctypes as C
import ctypes.util
from dataclasses import dataclass, fields
import os
import threading

__version__ = "0.0.8"

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
        "write_reads": ([C.c_void_p, C.POINTER(_Alignment), C.c_size_t, C.POINTER(C.c_size_t), C.c_size_t], C.c_int32),
        "write_read": ([C.c_void_p, C.POINTER(_Alignment), C.c_size_t], C.c_int32),
        "writer_finish": ([C.c_void_p], C.c_int32), "writer_destroy": ([C.c_void_p], C.c_int32),
        "reader_open": ([C.c_char_p, C.c_uint8, C.POINTER(C.c_void_p)], C.c_int32),
        "reader_kind": ([C.c_void_p], C.c_int32), "reader_destroy": ([C.c_void_p], C.c_int32),
        "reader_contigs": ([C.c_void_p, _ContigsCB, C.c_void_p], C.c_int32),
        "reader_next": ([C.c_void_p, _PairsCB, _ConcatCB, C.c_void_p], C.c_int32),
    }
    if hasattr(lib, "pqsio_stream_open"):
        signatures.update({
            "stream_open": ([C.c_char_p, C.c_uint8, C.c_uint64, C.c_uint32, C.c_uint32, C.POINTER(C.c_void_p)], C.c_int32),
            "stream_next": ([C.c_void_p, _PairsCB, _ConcatCB, C.c_void_p], C.c_int32),
            "stream_kind": ([C.c_void_p], C.c_int32),
            "stream_contigs": ([C.c_void_p, _ContigsCB, C.c_void_p], C.c_int32),
            "stream_destroy": ([C.c_void_p], C.c_int32),
        })
    parallel_signatures = {
        "parallel_open": ([C.c_char_p, C.c_uint32, C.POINTER(_Contig), C.c_size_t,
                           C.c_size_t, C.c_size_t, C.c_size_t, C.c_size_t, C.POINTER(C.c_void_p)], C.c_int32),
        "parallel_producer": ([C.c_void_p, C.POINTER(C.c_void_p)], C.c_int32),
        "producer_pairs": ([C.c_void_p, C.c_uint64, C.POINTER(_Pair), C.c_size_t], C.c_int32),
        "producer_reads": ([C.c_void_p, C.c_uint64, C.POINTER(_Alignment), C.c_size_t,
                            C.POINTER(C.c_size_t), C.c_size_t], C.c_int32),
        "parallel_finish": ([C.c_void_p], C.c_int32),
        "parallel_destroy": ([C.c_void_p], C.c_int32),
        "producer_destroy": ([C.c_void_p], C.c_int32),
    }
    if hasattr(lib, "pqsio_parallel_open"):
        signatures.update(parallel_signatures)
    for name, (args, result) in signatures.items():
        if name == "write_reads" and not hasattr(lib, "pqsio_write_reads"):
            continue  # Existing methods still work with ABI v1 from pqsio 0.0.1.
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

def _strand(value):
    if value not in ("+", "-"):
        raise ValueError("Strand must be + or -")
    return ord(value)

def _encode(row, cls):
    # The two ABI layouts are fixed. Avoid per-field reflection, dtype tests
    # and sizeof calls while keeping exactly the same validation as before.
    if cls is _Pair:
        return cls(_utf8(row.read_id), _uint(row.chrom1, 32), _uint(row.pos1, 64),
                   _uint(row.chrom2, 32), _uint(row.pos2, 64),
                   _strand(row.strand1), _strand(row.strand2), _uint(row.mapq, 8))
    if cls is _Alignment:
        return cls(_uint(row.read_idx, 64), _uint(row.read_length, 32),
                   _uint(row.read_start, 32), _uint(row.read_end, 32),
                   _strand(row.strand), _uint(row.chrom, 32),
                   _uint(row.start, 64), _uint(row.end, 64),
                   _uint(row.mapping_quality, 8), row.identity, _utf8(row.filter_reason))
    raise TypeError("Unsupported PQS record layout")

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

    def write_columns(self, batch):
        """Synchronously submit a typed PairColumns or ConcatColumns batch."""
        from .columns import write_columns
        return write_columns(self, batch)

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

    def write_batch(self, rows, read_offsets):
        """Submit flat alignments and offsets delimiting complete, ordered reads.

        Offsets start at 0 and end at len(rows). [0] denotes an empty batch.
        Invalid input is rejected before accepting any records in the batch.
        """
        self._active()
        if not hasattr(self._lib, "pqsio_write_reads"):
            raise RuntimeError("Bulk concat writing requires a pqsio >= 0.0.2 shared library")
        rows = list(rows)
        offsets = list(read_offsets)
        width = C.sizeof(C.c_size_t) * 8
        offsets = (C.c_size_t * len(offsets))(*[_uint(i, width) for i in offsets])
        batch = (_Alignment * len(rows))(*[_encode(r, _Alignment) for r in rows])
        _check(self._lib.pqsio_write_reads(self._handle, batch, len(batch), offsets, len(offsets)))

    def write_reads(self, reads):
        """Submit an iterable of complete reads in one native call.

        Batch size is controlled by the caller; the iterable is materialized.
        """
        rows, offsets = [], [0]
        for read in reads:
            rows.extend(read)
            offsets.append(len(rows))
        self.write_batch(rows, offsets)

class Producer:
    """Thread-safe batch submission; sequences start at 0 and have no gaps.

    A successful call queues data. Validation/I/O errors can surface at finish.
    Callers must keep the next expected sequence schedulable under backpressure.
    """
    def __init__(self, writer):
        writer._active()
        self._lib = writer._lib
        self._kind = writer._kind
        self._handle = C.c_void_p()
        self._lock = threading.Lock()
        self._calls = 0
        self._closed = False
        _check(self._lib.pqsio_parallel_producer(writer._handle, C.byref(self._handle)))

    def _submit(self, name, *args):
        with self._lock:
            if self._closed:
                raise RuntimeError("Producer is closed")
            self._calls += 1
            handle = self._handle
        try:
            _check(getattr(self._lib, name)(handle, *args))
        finally:
            with self._lock:
                self._calls -= 1
                if self._closed and self._calls == 0:
                    self._destroy()

    def write_batch(self, sequence, rows, read_offsets=None):
        sequence = _uint(sequence, 64)
        rows = list(rows)
        cls = _Pair if self._kind == 0 else _Alignment
        batch = (cls * len(rows))(*[_encode(r, cls) for r in rows])
        if self._kind == 0:
            if read_offsets is not None:
                raise ValueError("Pairs batches do not take read offsets")
            self._submit("pqsio_producer_pairs", sequence, batch, len(batch))
        else:
            if read_offsets is None:
                raise ValueError("Concat batches require complete-read offsets")
            offsets = list(read_offsets)
            width = C.sizeof(C.c_size_t) * 8
            offsets = (C.c_size_t * len(offsets))(*[_uint(i, width) for i in offsets])
            self._submit("pqsio_producer_reads", sequence, batch, len(batch), offsets, len(offsets))

    def write_reads(self, sequence, reads):
        if self._kind != 1:
            raise ValueError("write_reads requires concat")
        rows, offsets = [], [0]
        for read in reads:
            rows.extend(read)
            offsets.append(len(rows))
        self.write_batch(sequence, rows, offsets)

    def _destroy(self):
        if self._handle.value:
            self._lib.pqsio_producer_destroy(self._handle)
            self._handle = C.c_void_p()

    def close(self):
        # Defer freeing the native handle until concurrent calls have returned.
        with self._lock:
            self._closed = True
            if self._calls == 0:
                self._destroy()

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.close()

    def __del__(self):
        if hasattr(self, "_lock"):
            self.close()

class ParallelWriter(_Writer):
    """Owner of one parallel PQS output; owner lifecycle calls are exclusive.

    Share producers between threads. Join submitters before finish. Each batch
    ends a shard; max_batch_bytes bounds native input, not Python objects/RSS.
    """
    def __init__(self, path, contigs, kind="pairs", chunk_size=1_000_000,
                 workers=2, queue_capacity=4, max_batch_bytes=64 * 1024 * 1024):
        self._handle = C.c_void_p()
        self._lib = _library()
        if not hasattr(self._lib, "pqsio_parallel_open"):
            raise RuntimeError("Parallel writing requires a shared library with the parallel extension")
        if kind not in ("pairs", "concat"):
            raise ValueError("kind must be pairs or concat")
        self._kind = 0 if kind == "pairs" else 1
        entries = list(contigs.items()) if hasattr(contigs, "items") else list(contigs)
        cs = (_Contig * len(entries))(*[_Contig(_utf8(n), _uint(s, 64)) for n, s in entries])
        width = C.sizeof(C.c_size_t) * 8
        values = [_uint(v, width) for v in (chunk_size, workers, queue_capacity, max_batch_bytes)]
        _check(self._lib.pqsio_parallel_open(_utf8(os.fspath(path)), self._kind, cs, len(cs),
                                            *values, C.byref(self._handle)))

    def producer(self):
        return Producer(self)

    def finish(self):
        self._active()
        try:
            _check(self._lib.pqsio_parallel_finish(self._handle))
        finally:
            self.close()

    def close(self):
        if self._handle.value:
            self._lib.pqsio_parallel_destroy(self._handle)
            self._handle = C.c_void_p()

class Reader:
    _prefix = "reader"
    def __init__(self, path, min_mapq=0):
        self._handle = C.c_void_p()
        self._lib = _library()
        _check(self._lib.pqsio_reader_open(_utf8(os.fspath(path)), _uint(min_mapq, 8), C.byref(self._handle)))
        self._metadata()

    def _metadata(self):
        try:
            self.kind = ("pairs", "concat")[_check(getattr(self._lib, "pqsio_" + self._prefix + "_kind")(self._handle))]
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
            code = getattr(self._lib, "pqsio_" + self._prefix + "_contigs")(self._handle, callback, None)
            if errors:
                raise errors[0]
            _check(code)
        except BaseException:
            self.close()
            raise
    def iter_columns(self):
        """Yield independent array-backed batches, one per filtered shard."""
        from .columns import iter_columns
        return iter_columns(self)

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
            code = getattr(self._lib, "pqsio_" + self._prefix + "_next")(self._handle, pairs, concat, None)
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
            getattr(self._lib, "pqsio_" + self._prefix + "_destroy")(self._handle)
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

from .columns import PairColumns, ConcatColumns, pack_strings


class StreamingReader(Reader):
    """NEW: row-group streaming, independent of disk shard size.

    batch_rows controls output rows, not bytes. boundary and filter_mode are
    independent. None means matching_alignments for concat; pairs requires None.
    CompleteReads filtering returns q0 records of reads with any matching row.
    """
    _prefix = "stream"

    def __init__(self, path, min_mapq=0, *, batch_rows=65536,
                 boundary="rows", filter_mode=None):
        self._handle = C.c_void_p()
        self._lib = _library()
        if not hasattr(self._lib, "pqsio_stream_open"):
            raise RuntimeError("Native pqsio library lacks streaming capability; rebuild/update it. Legacy Reader remains supported.")
        batch_rows = _uint(batch_rows, 32)
        if batch_rows == 0:
            raise ValueError("batch_rows must be in 1..=4294967295")
        boundaries = {"rows": 0, "complete_reads": 1}
        filters = {None: 0, "matching_alignments": 1, "complete_reads": 2}
        if boundary not in boundaries or filter_mode not in filters:
            raise ValueError("Invalid streaming boundary or filter_mode")
        _check(self._lib.pqsio_stream_open(_utf8(os.fspath(path)), _uint(min_mapq, 8),
               batch_rows, boundaries[boundary], filters[filter_mode], C.byref(self._handle)))
        self._metadata()

    def iter_columns(self):
        """Yield owned column batches honoring streaming boundary/filter options."""
        return super().iter_columns()

    def iter_reads(self):
        raise NotImplementedError("Use iter_batches with boundary='complete_reads'; batches may contain multiple reads")

from .inspection import Metadata, Inspection, ValidationReport, inspect, validate
