"""Native summaries and text export; Python never iterates dataset records."""
import ctypes as C
import json
import os

from .convert import ConvertResult
from .inspection import _Callback


def _invoke(symbol, args, types):
    from . import _library, _utf8, _check
    lib = _library()
    if not hasattr(lib, symbol):
        raise RuntimeError(f'Native pqsio library lacks {symbol}; rebuild/update it')
    fn = getattr(lib, symbol)
    fn.argtypes = types + [_Callback, C.c_void_p]
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

    args = [None if v is None else _utf8(os.fsdecode(v)) if t is C.c_char_p else v
            for v, t in zip(args, types)]
    code = fn(*args, receive, None)
    if errors:
        raise errors[0]
    if code == -3:
        raise BrokenPipeError('output pipe closed')
    _check(code)
    return ConvertResult(result[0])


def info(path, *, stats=False):
    """Read metadata/footers; stats=True additionally scans q0 for exact MAPQ/read counts."""
    if not isinstance(stats, bool):
        raise ValueError('stats must be bool')
    return _invoke('pqsio_info_json', [path, int(stats)], [C.c_char_p, C.c_uint32])


def stats(path, *, min_mapq=0):
    """Scan q0 once for selected-record quality metrics; q1 is not added."""
    if isinstance(min_mapq, bool) or not isinstance(min_mapq, int) or not 0 <= min_mapq <= 255:
        raise ValueError('min_mapq must be an integer in 0..=255')
    return _invoke('pqsio_stats_json', [path, min_mapq], [C.c_char_p, C.c_uint8])


def index_status(path, *, quality='q0'):
    """Check source footers and index checksums without decoding record pages."""
    if quality not in ('q0', 'q1'):
        raise ValueError('quality must be q0 or q1')
    return _invoke('pqsio_index_status_json', [path, int(quality == 'q1')], [C.c_char_p, C.c_uint32])


def export(input, output=None, *, format='auto', columns=None, limit=None,
           min_mapq=0, regions=None, pairs_mode='either', header=True,
           threads=1, batch_rows=65_536, index='auto', filter_mode=None, auto_index=False):
    """Rust text export; output=None or '-' writes uncompressed data to OS stdout.

    auto chooses pairs/concat text for the corresponding input, or TSV
    when selecting columns. Concat text has all 11 columns and no header. File suffix .gz/.mgz enables gzip-compatible compression.
    Limits count rows; concat reads can be split. No existing file is replaced.
    """
    if format not in ('auto', 'pairs', 'concat', 'tsv'):
        raise ValueError('format must be auto, pairs, concat or tsv')
    if pairs_mode not in ('either', 'both') or not isinstance(header, bool):
        raise ValueError('pairs_mode must be either/both and header must be bool')
    if index not in ('auto', 'off', 'require'):
        raise ValueError('index must be auto, off or require')
    if not isinstance(auto_index, bool):
        raise ValueError('auto_index must be bool')
    if filter_mode not in (None, 'matching_alignments', 'complete_reads'):
        raise ValueError('filter_mode must be matching_alignments or complete_reads')
    size_max = C.c_size_t(-1).value
    for name, value, lower, upper in [
        ('limit', limit, 0, 2**64-1), ('min_mapq', min_mapq, 0, 255),
        ('threads', threads, 1, size_max), ('batch_rows', batch_rows, 1, min(size_max, 2**32-1)),
    ]:
        if name == 'limit' and value is None:
            continue
        if isinstance(value, bool) or not isinstance(value, int) or not lower <= value <= upper:
            raise ValueError(f'{name} must be an integer in {lower}..={upper}')
    if columns is not None and (not isinstance(columns, (list, tuple)) or not columns
                               or any(not isinstance(c, str) or not c for c in columns)
                               or len(set(columns)) != len(columns)):
        raise ValueError('columns must be a nonempty list of distinct column names')
    if regions is None:
        regions = []
    if not isinstance(regions, (list, tuple)):
        raise ValueError('regions must be a list of (contig, start, end)')
    for region in regions:
        if (not isinstance(region, (list, tuple)) or len(region) != 3
                or not isinstance(region[0], str)
                or any(isinstance(v, bool) or not isinstance(v, int) for v in region[1:])
                or not 0 <= region[1] < region[2] <= 2**64-1):
            raise ValueError('regions require (contig, 0 <= start < end <= UInt64 maximum)')
    if not regions and index != 'auto':
        raise ValueError('index mode requires at least one region')
    options = dict(format=format, columns=columns, limit=limit, min_mapq=min_mapq,
                   regions=regions, pairs_mode=pairs_mode, header=header,
                   threads=threads, batch_rows=batch_rows, index=index, filter_mode=filter_mode,
                   auto_index=auto_index)
    if output is not None and os.fsdecode(output) == '-':
        output = None
    return _invoke('pqsio_export_json', [input, output, json.dumps(options)], [C.c_char_p]*3)


def view(input, *, limit=100, columns=None, **options):
    """Preview TSV directly on OS stdout; set limit=None for all matching rows."""
    return export(input, None, format='tsv', limit=limit, columns=columns, **options)
