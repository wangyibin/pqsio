"""Select complete concat reads and optionally expand them, using native pqsio.

Run from the repository with `pixi run python examples/concat_workflow.py --help`.
"""
import argparse
import json
from pathlib import Path
import sys

import pqsio as p


def region(value):
    try:
        chrom, interval = value.rsplit(':', 1)
        start, end = map(int, interval.split('-'))
        if not chrom or not 0 <= start < end <= 2**64 - 1:
            raise ValueError
        return chrom, start, end
    except ValueError:
        raise argparse.ArgumentTypeError('use CHROM:START-END with 0 <= START < END')


def integer(minimum, maximum):
    def parse(value):
        try:
            number = int(value)
        except ValueError:
            raise argparse.ArgumentTypeError('expected an integer')
        if not minimum <= number <= maximum:
            raise argparse.ArgumentTypeError(f'expected {minimum}..{maximum}')
        return number
    return parse


def demo(path):
    # Read 7 has an anchor, a distant high-quality fragment, and a low-quality
    # fragment. Read 9 has only a low-quality anchor; read 11 has no anchor.
    with p.ConcatWriter(path, {'chr1': 1000, 'chr2': 1000}, chunk_size=2) as writer:
        writer.write_read([
            p.Alignment(7, 300, 100, 200, '-', 1, 300, 350, 40, .75),
            p.Alignment(7, 300, 0, 100, '+', 0, 100, 150, 60, 1.),
            p.Alignment(7, 300, 200, 300, '+', 0, 800, 850, 0, .5, 'low'),
        ])
        writer.write_read([
            p.Alignment(9, 200, 0, 100, '+', 0, 100, 150, 10, 1.),
            p.Alignment(9, 200, 100, 200, '+', 1, 500, 550, 60, 1.),
        ])
        writer.write_read([
            p.Alignment(11, 200, 0, 100, '+', 0, 600, 650, 60, 1.),
            p.Alignment(11, 200, 100, 200, '+', 1, 700, 750, 60, 1.),
        ])
    p.set_copy_numbers(path, {'chr1': 2})


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument('--input', type=Path, help='Existing concat PQS directory')
    source.add_argument('--demo', action='store_true', help='Create a tiny synthetic input')
    parser.add_argument('--output', type=Path, required=True, help='New workflow directory')
    parser.add_argument('--region', type=region, action='append', help='Anchor region; repeat for union')
    parser.add_argument('--read-index', type=integer(0, 2**64-1), action='append',
                        help='Concat logical ID; intersects region/MAPQ selection')
    parser.add_argument('--select-mapq', type=integer(0, 255), default=30,
                        help='Minimum MAPQ of the alignment selecting a read (default: 30)')
    parser.add_argument('--expand', action='store_true', help='Also create pairs.pqs')
    parser.add_argument('--pair-mapq', type=integer(0, 255), default=30,
                        help='Minimum alignment MAPQ during expansion (default: 30)')
    parser.add_argument('--min-order', type=integer(2, sys.maxsize), default=2,
                        help='Minimum alignment count after pair-MAPQ filtering (default: 2)')
    parser.add_argument('--max-order', type=integer(3, sys.maxsize),
                        help='Exclusive upper alignment count after pair-MAPQ filtering')
    parser.add_argument('--threads', type=integer(1, sys.maxsize), default=1,
                        help='Expansion workers (default: 1)')
    args = parser.parse_args(argv)
    if args.max_order is not None and args.max_order <= args.min_order:
        parser.error('--max-order must exceed --min-order (exclusive upper bound)')
    regions = args.region
    if args.demo and regions is None:
        regions = [('chr1', 100, 200)]
    try:
        # Reject the wrong input kind before creating workflow output.
        if args.input is not None:
            input_path = args.input.resolve()
            output_path = args.output.resolve()
            if (input_path == output_path or input_path in output_path.parents
                    or output_path in input_path.parents):
                raise ValueError('--output must not overlap the input dataset')
            with p.Reader(args.input) as reader:
                if reader.kind != 'concat':
                    raise ValueError('--input must be a concat PQS dataset')
        args.output.mkdir()  # Never overwrite a previous run, even an empty directory.
        input_path = args.input or args.output / 'input.concat.pqs'
        if args.demo:
            demo(input_path)
        selected = args.output / 'selected.concat.pqs'
        selection = p.subset(input_path, selected, regions=regions,
                             read_ids=args.read_index, min_mapq=args.select_mapq,
                             mode='complete_reads').to_dict()
        expansion = None
        if args.expand:
            expansion = p.convert(selected, args.output / 'pairs.pqs',
                                  mode='concat2pairs', min_mapq=args.pair_mapq,
                                  min_order=args.min_order, max_order=args.max_order,
                                  threads=args.threads).to_dict()
        report = dict(input=str(input_path), selection=selection, expansion=expansion,
                      select_mapq=args.select_mapq, pair_mapq=args.pair_mapq if args.expand else None,
                      min_order=args.min_order if args.expand else None,
                      max_order=args.max_order if args.expand else None)
        payload = json.dumps(report, indent=2, ensure_ascii=False)
        (args.output / 'workflow.json').write_text(payload + '\n', encoding='utf-8')
        print(payload)
    except (OSError, ValueError, RuntimeError) as error:
        parser.exit(1, f'error: {error}\nCompleted datasets, if any, remain in {args.output}; '
                       'use a new output directory for another run.\n')


if __name__ == '__main__':
    main()
