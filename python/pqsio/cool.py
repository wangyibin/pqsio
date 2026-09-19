"""Single-call binding for native Rust pairs-to-Cooler conversion."""
import ctypes as C
from decimal import Decimal, DecimalException, localcontext
import json
import os
from .inspection import _Callback


def _parse_bin_size(value):
    """CPhasing-style decimal bp units, with exact Int64 range validation."""
    message = 'bin_size must be positive bp in 1..=9223372036854775807 (e.g. 10000, 10k, 1m or 1.5m)'
    if value is None:
        raise ValueError('pairs2cool requires --bin-size (e.g. 10000, 10k or 1m)')
    if isinstance(value, str):
        text = value.lower().replace(' ', '')
        power = {'k': 3, 'm': 6, 'g': 9}.get(text[-1:], 0)
        if power:
            text = text[:-1]
        try:
            number = Decimal(text)
            if not number.is_finite():
                raise ValueError(message)
            with localcontext() as context:
                context.prec = max(28, len(number.as_tuple().digits))
                number = number.scaleb(power)
            if not 1 <= number < 2**63:
                raise ValueError(message)
            # CPhasing truncates fractional bp after applying the unit.
            value = int(number)
        except (DecimalException, ValueError):
            raise ValueError(message) from None
    if isinstance(value, bool) or not isinstance(value, int) or not 1 <= value <= 2**63-1:
        raise ValueError(message)
    return value


def pairs2cool(input, output, *, bin_size, chunk_size, batch_rows, min_mapq,
               threads, contigsizes, tmpdir):
    from . import _library, _utf8, _check
    bin_size = _parse_bin_size(bin_size)
    size_max = C.c_size_t(-1).value
    for name, value, minimum, maximum in [
        ('chunk_size', chunk_size, 1, size_max),
        ('batch_rows', batch_rows, 1, min(2**32-1, size_max)),
        ('min_mapq', min_mapq, 0, 255), ('threads', threads, 1, size_max),
    ]:
        if isinstance(value, bool) or not isinstance(value, int) or not minimum <= value <= maximum:
            raise ValueError(f'{name} must be an integer in {minimum}..={maximum}')
    lib = _library()
    if not hasattr(lib, 'pqsio_pairs2cool_json'):
        raise RuntimeError('Native pqsio library lacks pairs2cool capability; rebuild/update it')
    fn = lib.pqsio_pairs2cool_json
    fn.argtypes = [C.c_char_p, C.c_char_p, C.c_uint64, C.c_size_t, C.c_size_t,
                   C.c_uint8, C.c_size_t, C.c_char_p, C.c_char_p, _Callback, C.c_void_p]
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

    code = fn(path(input), path(output), bin_size, chunk_size, batch_rows,
              min_mapq, threads, path(contigsizes), path(tmpdir), receive, None)
    if errors:
        raise errors[0]
    if code == -2:
        raise ValueError(lib.pqsio_last_error().decode('utf-8'))
    _check(code)
    return result[0]
