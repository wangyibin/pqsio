"""PQS/Cooler conversion and BAM/PAF import."""
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
            batch_rows=65_536, min_mapq=0, min_order=None, max_order=None, threads=1,
            contigsizes=None, include_secondary=False, samtools=None, tmpdir=None,
            pair_position=None, bin_size=None):
    """Convert concat/BAM/PAF to PQS, or pairs PQS/text to Cooler.

    Modes: concat2pairs, bam2pairs, bam2concat, paf2pairs, paf2concat, pairs2cool.
    pairs2cool requires bin_size (bp integer or string such as '10k', '1m')
    and writes a single-resolution .cool file.
    Order filtering follows MAPQ filtering; max_order is exclusive. The default min_order is 2
    for pairs output and 1 for concat imports. All conversions execute in Rust.
    See docs/import.md and docs/cool.md for coordinates and filtering rules.
    """
    from . import _library, _utf8, _check
    if mode not in ('concat2pairs', 'bam2pairs', 'bam2concat', 'paf2pairs', 'paf2concat', 'pairs2cool'):
        raise ValueError('mode must be concat2pairs, bam2pairs, bam2concat, paf2pairs, paf2concat or pairs2cool')
    if not isinstance(include_secondary, bool):
        raise ValueError('include_secondary must be bool')
    if mode == 'pairs2cool':
        if include_secondary or any(v is not None for v in (min_order, max_order, samtools, pair_position)):
            raise ValueError('pairs2cool does not accept alignment-order, secondary, samtools or pair-position options')
        from .cool import pairs2cool
        return ConvertResult(pairs2cool(input, output, bin_size=bin_size,
            chunk_size=chunk_size, batch_rows=batch_rows, min_mapq=min_mapq,
            threads=threads, contigsizes=contigsizes, tmpdir=tmpdir))
    if bin_size is not None:
        raise ValueError('bin_size is supported only for pairs2cool')
    if mode == 'concat2pairs' and any(value is not None for value in
                                     (contigsizes, samtools, tmpdir, pair_position)):
        raise ValueError('BAM/PAF import options cannot be used with concat2pairs')
    if include_secondary and mode == 'concat2pairs':
        raise ValueError('include_secondary is supported only for BAM/PAF imports')
    if contigsizes is not None and mode not in ('paf2pairs', 'paf2concat'):
        raise ValueError('contigsizes is supported only for PAF imports; BAM uses its header')
    if samtools is not None and mode not in ('bam2concat', 'bam2pairs'):
        raise ValueError('samtools is supported only for BAM imports')
    if pair_position is not None and mode not in ('bam2pairs', 'paf2pairs'):
        raise ValueError('pair_position is supported only for bam2pairs and paf2pairs')
    if pair_position not in (None, 'five-prime', 'leftmost'):
        raise ValueError('pair_position must be five-prime or leftmost')
    minimum_order = 2 if mode in ('concat2pairs', 'bam2pairs', 'paf2pairs') else 1
    if min_order is None:
        min_order = minimum_order
    size_max = C.c_size_t(-1).value
    for name, value, minimum, maximum in [
        ('threads', threads, 1, size_max),
        ('chunk_size', chunk_size, 1, size_max),
        ('batch_rows', batch_rows, 1, min(2**32-1, size_max)),
        ('min_mapq', min_mapq, 0, 255),
        ('min_order', min_order, minimum_order, size_max),
        ('max_order', max_order if max_order is not None else size_max, minimum_order+1, size_max),
    ]:
        if isinstance(value, bool) or not isinstance(value, int) or not minimum <= value <= maximum:
            raise ValueError(f'{name} must be an integer in {minimum}..={maximum}')
    if max_order is not None and max_order <= min_order:
        raise ValueError('max_order must exceed min_order (exclusive upper bound)')
    if mode != 'concat2pairs':
        from .import_alignments import import_alignments
        return ConvertResult(import_alignments(
            input, output, mode, chunk_size=chunk_size, batch_rows=batch_rows,
            min_mapq=min_mapq, min_order=min_order, max_order=max_order, threads=threads,
            contigsizes=contigsizes, include_secondary=include_secondary,
            samtools=samtools, tmpdir=tmpdir, pair_position=pair_position or 'leftmost'))
    if max_order is None:
        max_order = size_max
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
