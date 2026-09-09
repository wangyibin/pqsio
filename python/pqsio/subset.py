"""Order-preserving native q0 subset; standard-library-only binding."""
import copy
import ctypes as C
import json
import os
from dataclasses import dataclass
from .inspection import _Callback


@dataclass(frozen=True)
class SubsetResult:
    _data: dict

    def to_dict(self):
        return copy.deepcopy(self._data)


def subset(input, output, *, min_mapq=None, chroms=None, regions=None,
           read_ids=None, pairs_mode=None, mode=None, batch_rows=65_536,
           chunk_size=1_000_000, provenance=True):
    """Select q0 records, preserving fields, order, contigs and logical IDs.

    None disables a condition; an empty list matches nothing. Regions are
    0-based half-open. Concat mode defaults to matching_alignments; complete_reads
    retains all q0 alignments of a read with one fully matching alignment.
    read_ids accepts lists/tuples (100,000 entries, 4 MiB string payload limit).
    Memory includes a decoded row group, batches, Writer buffers and the largest
    read. Inputs must remain unchanged. See docs/subset.md for full contracts.
    """
    from . import _library, _utf8, _check

    def uint(name, value, maximum, minimum=0):
        if isinstance(value, bool) or not isinstance(value, int) or not minimum <= value <= maximum:
            raise ValueError(f'{name} must be an integer in {minimum}..={maximum}')

    if min_mapq is not None:
        uint('min_mapq', min_mapq, 255)
    uint('batch_rows', batch_rows, min(2**32-1, C.c_size_t(-1).value), 1)
    uint('chunk_size', chunk_size, C.c_size_t(-1).value, 1)
    if not isinstance(provenance, bool):
        raise TypeError('provenance must be bool')
    if pairs_mode not in (None, 'either', 'both'):
        raise ValueError('pairs_mode must be either or both')
    if mode not in (None, 'matching_alignments', 'complete_reads'):
        raise ValueError('mode must be matching_alignments or complete_reads')
    for name, values in [('chroms', chroms), ('regions', regions), ('read_ids', read_ids)]:
        if values is not None and not isinstance(values, (list, tuple)):
            raise TypeError(f'{name} must be a list or tuple, or None')
    if chroms is not None and any(not isinstance(s, str) for s in chroms):
        raise TypeError('chroms must contain strings')
    if regions is not None:
        for r in regions:
            if not isinstance(r, (list, tuple)) or len(r) != 3 or not isinstance(r[0], str):
                raise TypeError('region must be (contig, start, end)')
            uint('region start', r[1], 2**64-1)
            uint('region end', r[2], 2**64-1)
    if read_ids is not None:
        if len(read_ids) > 100_000:
            raise ValueError('read_ids exceeds 100000 entries')
        size = 0
        for value in read_ids:
            if isinstance(value, str):
                size += len(value.encode('utf-8'))
                if size > 4 * 1024 * 1024:
                    raise ValueError('read_ids exceeds 4 MiB UTF-8 bytes')
            else:
                uint('read ID', value, 2**64-1)
    options = dict(min_mapq=min_mapq, chroms=chroms, regions=regions,
                   read_ids=read_ids, pairs_mode=pairs_mode, mode=mode,
                   batch_rows=batch_rows, chunk_size=chunk_size, provenance=provenance)
    payload = json.dumps(options, ensure_ascii=False, separators=(',', ':')).encode('utf-8')
    if len(payload) > 16 * 1024 * 1024:
        raise ValueError('subset options exceeds 16 MiB limit')
    lib = _library()
    if not hasattr(lib, 'pqsio_subset_json'):
        raise RuntimeError('Native pqsio library lacks subset capability; rebuild/update it')
    fn = lib.pqsio_subset_json
    fn.argtypes = [C.c_char_p, C.c_char_p, C.c_char_p, _Callback, C.c_void_p]
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

    code = fn(_utf8(os.fspath(input)), _utf8(os.fspath(output)), payload, receive, None)
    if errors:
        raise errors[0]
    _check(code)
    return SubsetResult(result[0])
