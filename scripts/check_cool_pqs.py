#!/usr/bin/env python3
"""Opt-in full independent PQS-to-Cooler audit (PyArrow/NumPy/h5py/cooler).

Reads every q0 record, independently bins and aggregates contacts, then checks
every output pixel and both indexes. Holds accepted contact keys in memory;
intended for explicit validation, not normal conversion. Never modifies inputs.
"""
import argparse
import hashlib
import json
from pathlib import Path
import time
import zlib

import cooler
import h5py
import numpy as np
import pyarrow as pa
import pyarrow.parquet as pq


def require(condition, message):
    if not condition:
        raise ValueError(message)


def manifest(source):
    return {
        'metadata': {name: hashlib.sha256((source/name).read_bytes()).hexdigest()
                     for name in ['_metadata', '_metadata_counts', '_contigsizes']},
        'shards': [(p.name, p.stat().st_size, p.stat().st_mtime_ns)
                   for p in sorted((source/'q0').glob('*.parquet'))],
    }


def audit(source, output, bin_size, min_mapq):
    started = time.monotonic()
    before = manifest(source)
    output_before = (output.stat().st_size, output.stat().st_mtime_ns)
    sizes = [line.split() for line in (source/'_contigsizes').read_text().splitlines() if line.strip()]
    names = [row[0] for row in sizes]
    lengths = np.array([int(row[1]) for row in sizes], dtype=np.int64)
    ids = {name: i for i, name in enumerate(names)}
    require(len(ids) == len(names) and np.all(lengths > 0), 'invalid contig dictionary')
    chrom_bins = (lengths-1)//bin_size+1
    offsets = np.r_[0, np.cumsum(chrom_bins)]
    nbins = int(offsets[-1])
    require(nbins > 0 and nbins*nbins <= np.iinfo(np.uint64).max, 'too many bins for audit key')
    recorded = dict(line.split() for line in (source/'_metadata_counts').read_text().splitlines() if line.strip())
    capacity = int(recorded['q0_records'])
    keys = np.empty(capacity, dtype=np.uint64)
    scanned = accepted = boundary_positions = 0
    for index, (name, _, _) in enumerate(before['shards']):
        parquet = pq.ParquetFile(source/'q0'/name)
        for batch in parquet.iter_batches(batch_size=500_000,
                                          columns=['chrom1', 'pos1', 'chrom2', 'pos2', 'mapq']):
            columns = {name: batch.column(i) for i, name in enumerate(batch.schema.names)}
            require(all(c.null_count == 0 for c in columns.values()), 'null input column')
            keep = columns['mapq'].to_numpy(zero_copy_only=False) >= min_mapq
            endpoints = []
            for suffix in ['1', '2']:
                chrom = columns['chrom'+suffix]
                if not pa.types.is_dictionary(chrom.type):
                    chrom = chrom.dictionary_encode()
                lookup = np.array([ids[n] for n in chrom.dictionary.to_pylist()], dtype=np.int64)
                chrom_ids = lookup[chrom.indices.to_numpy(zero_copy_only=False)][keep]
                positions = columns['pos'+suffix].to_numpy(zero_copy_only=False)[keep].astype(np.int64)
                require(np.all((positions > 0) & (positions <= lengths[chrom_ids])), 'coordinate outside contig')
                boundary_positions += int(np.count_nonzero(positions % bin_size == 0))
                # Pairs coordinates are one-based; bins are zero-based half-open.
                endpoints.append(offsets[chrom_ids] + (positions-1)//bin_size)
            a, b = endpoints
            count = len(a)
            require(accepted+count <= capacity, 'input count exceeds metadata')
            keys[accepted:accepted+count] = np.minimum(a, b).astype(np.uint64)*nbins + np.maximum(a, b).astype(np.uint64)
            accepted += count
            scanned += batch.num_rows
        if (index+1) % 25 == 0:
            print('Read {}/{} shards, {} accepted contacts'.format(index+1, len(before['shards']), accepted), flush=True)
    require(scanned == capacity, 'q0 total differs from metadata')
    if min_mapq in [0, 1]:
        require(accepted == int(recorded['q{}_records'.format(min_mapq)]), 'accepted total differs from metadata')
    keys.resize(accepted, refcheck=False)
    print('Sorting and aggregating {} independently binned contacts'.format(accepted), flush=True)
    keys.sort()
    unique, counts = np.unique(keys, return_counts=True)
    del keys
    nnz = len(unique)
    histogram = np.zeros(nbins, dtype=np.int64)
    digest = hashlib.sha256()
    with h5py.File(output, 'r') as f:
        expected_attrs = {'format': 'HDF5::Cooler', 'format-version': 3,
                          'bin-type': 'fixed', 'storage-mode': 'symmetric-upper',
                          'bin-size': bin_size, 'nchroms': len(names), 'nbins': nbins,
                          'nnz': nnz, 'sum': accepted}
        for name, expected in expected_attrs.items():
            value = f.attrs[name]
            if isinstance(value, bytes):
                value = value.decode()
            require(value == expected, 'attribute mismatch: '+name)
        require([x.decode() for x in f['chroms/name'][:]] == names, 'chromosome order mismatch')
        require(np.array_equal(f['chroms/length'][:], lengths), 'chromosome lengths mismatch')
        require(np.array_equal(f['indexes/chrom_offset'][:], offsets), 'chrom_offset mismatch')
        for name in ['chrom', 'start', 'end']:
            require(f['bins/'+name].shape == (nbins,), 'bin column length mismatch')
        for chrom, length in enumerate(lengths):
            lo, hi = offsets[chrom:chrom+2]
            starts = np.arange(chrom_bins[chrom])*bin_size
            require(np.all(f['bins/chrom'][lo:hi] == chrom), 'bin chromosome mismatch')
            require(np.array_equal(f['bins/start'][lo:hi], starts), 'bin start mismatch')
            require(np.array_equal(f['bins/end'][lo:hi], np.minimum(starts+bin_size, length)), 'bin end mismatch')
        for name in ['bin1_id', 'bin2_id', 'count']:
            d = f['pixels/'+name]
            require(d.shape == (nnz,) and d.dtype.kind == 'i' and d.dtype.itemsize == 8, 'pixel shape/type mismatch')
            require(d.shuffle and d.compression == 'gzip' and d.compression_opts == 6, 'pixel filters mismatch')
            if nnz and nnz % d.chunks[0]:
                chunk = d.chunks[0]
                last = nnz//chunk*chunk
                mask, compressed = d.id.read_direct_chunk((last,))
                require(mask == 0, 'unexpected filter mask')
                raw = zlib.decompress(compressed)
                require(len(raw) == chunk*8, 'partial chunk physical size mismatch')
                values = np.frombuffer(np.frombuffer(raw, dtype='u1').reshape(8, chunk).T.copy().tobytes(), dtype=d.dtype)
                require(np.array_equal(values[:nnz-last], d[last:]), 'raw tail chunk mismatch')
                require(np.all(values[nnz-last:] == 0), 'nonzero tail padding')
        for lo in range(0, nnz, 1_000_000):
            hi = min(lo+1_000_000, nnz)
            a, b = unique[lo:hi]//nbins, unique[lo:hi] % nbins
            expected = np.column_stack([a, b, counts[lo:hi]]).astype('<i8')
            actual = np.column_stack([f['pixels/'+name][lo:hi] for name in ['bin1_id', 'bin2_id', 'count']])
            require(np.array_equal(actual, expected), 'pixel mismatch at rows {}:{}'.format(lo, hi))
            histogram += np.bincount(a.astype(np.int64), minlength=nbins)
            digest.update(expected.tobytes())
        require(np.array_equal(f['indexes/bin1_offset'][:], np.r_[0, np.cumsum(histogram)]), 'bin1_offset mismatch')
    # Exercise the standard Cooler matrix API, including symmetric reflection.
    reader = cooler.Cooler(str(output))
    windows = []
    for start in sorted(set([0, nbins//2, max(0, nbins-64), max(0, int(histogram.argmax())-32)])):
        stop = min(start+64, nbins)
        lo, hi = np.searchsorted(unique, [start*nbins, stop*nbins])
        a, b = unique[lo:hi]//nbins, unique[lo:hi] % nbins
        keep = (b >= start) & (b < stop)
        x, y = (a[keep]-start).astype(np.int64), (b[keep]-start).astype(np.int64)
        values = counts[lo:hi][keep]
        expected = np.zeros((stop-start, stop-start), dtype=np.int64)
        expected[x, y] = values
        expected[y, x] = values
        require(np.array_equal(reader.matrix(balance=False)[start:stop, start:stop], expected), 'Cooler matrix API mismatch')
        windows.append([start, stop])
    require(manifest(source) == before, 'input manifest changed')
    require((output.stat().st_size, output.stat().st_mtime_ns) == output_before, 'output changed during audit')
    return dict(passed=True, input=str(source), output=str(output), bin_size=bin_size,
                min_mapq=min_mapq, input_records=scanned, accepted_contacts=accepted,
                independent_nnz=nnz, pixels_compared=nnz, exact_boundary_endpoints=boundary_positions,
                nchroms=len(names), nbins=nbins, matrix_windows=windows,
                pixel_sha256=digest.hexdigest(), output_bytes=output_before[0],
                output_sha256=hashlib.sha256(output.read_bytes()).hexdigest(),
                input_manifest=before, elapsed_seconds=time.monotonic()-started,
                versions={m.__name__: m.__version__ for m in [pa, np, h5py, cooler]})


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('input', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--bin-size', type=int, required=True)
    parser.add_argument('--min-mapq', type=int, default=1)
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    require(args.bin_size > 0 and 0 <= args.min_mapq <= 255, 'invalid bin-size or min-mapq')
    require(not args.report.exists(), 'report already exists')
    result = audit(args.input.resolve(), args.output.resolve(), args.bin_size, args.min_mapq)
    with args.report.open('x') as stream:
        json.dump(result, stream, indent=2)
        stream.write('\n')
    print(json.dumps({k: v for k, v in result.items() if k != 'input_manifest'}), flush=True)
