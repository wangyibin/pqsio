"""Rich-click command-line access to native PQS operations."""
import ctypes as C
import copy
import json
import re

import rich_click as click

from . import (__version__, convert, inspect, merge, subset, validate, info, view,
              export, stats, index_status, build_index)
from .convert import ConvertResult
from .cool import _parse_bin_size


_SIZE_MAX = C.c_size_t(-1).value
_POSITIVE = click.IntRange(1, _SIZE_MAX)
_BATCH_ROWS = click.IntRange(1, min(2**32-1, _SIZE_MAX))
_MAPQ = click.IntRange(0, 255)
_CONTEXT = {'help_option_names': ['-h', '--help', '-help']}

# Match CPhasing's help layout while keeping explicit help on stdout.
click.rich_click.USE_MARKDOWN = True
click.rich_click.STYLE_COMMANDS_TABLE_SHOW_LINES = False
click.rich_click.STYLE_COMMANDS_TABLE_PAD_EDGE = True
click.rich_click.STYLE_COMMANDS_TABLE_BOX = 'SIMPLE'
click.rich_click.STYLE_COMMANDS_TABLE_BORDER_STYLE = 'red'
click.rich_click.STYLE_USAGE_COMMAND = 'bold red'
click.rich_click.MAX_WIDTH = 128
click.rich_click.SHOW_ARGUMENTS = True
click.rich_click.APPEND_METAVARS_HELP = True
click.rich_click.SHOW_METAVARS_COLUMN = False
click.rich_click.STYLE_ERRORS_SUGGESTION = 'magenta italic'
click.rich_click.ERRORS_SUGGESTION = "Try running the '--help' flag for more information."
click.rich_click.COMMAND_GROUPS = {
    'pqsio': [
        {'name': 'Conversion', 'commands': ['bam2pairs', 'bam2concat', 'paf2pairs', 'paf2concat', 'concat2pairs', 'pairs2cool', 'convert']},
        {'name': 'Browse', 'commands': ['info', 'stats', 'head', 'view', 'query']},
        {'name': 'Dataset information', 'commands': ['inspect', 'validate']},
        {'name': 'Dataset operations', 'commands': ['export', 'subset', 'merge', 'index']},
    ],
}
click.rich_click.OPTION_GROUPS = {
    'pqsio convert': [
        {'name': 'Conversion', 'options': ['--mode', '--output']},
        {'name': 'Filtering', 'options': ['--min-mapq', '--min-order', '--max-order']},
        {'name': 'Performance', 'options': ['--threads', '--chunk-size', '--batch-rows', '--tmpdir']},
        {'name': 'Alignment input', 'options': ['--contigsizes', '--include-secondary', '--pair-position']},
        {'name': 'Cooler output', 'options': ['--bin-size']},
    ],
    'pqsio subset': [
        {'name': 'Selection', 'options': ['--min-mapq', '--chrom', '--region', '--read-id', '--read-index']},
        {'name': 'Selection mode', 'options': ['--pairs-mode', '--mode']},
        {'name': 'Output', 'options': ['--output', '--chunk-size', '--batch-rows', '--no-provenance']},
    ],
}


class _RegionType(click.ParamType):
    name = 'region'

    def convert(self, value, param, ctx):
        match = re.fullmatch(r'(.+):([0-9]+)-([0-9]+)', value)
        if match:
            chrom, start, end = match.groups()
            try:
                start, end = int(start), int(end)
                if 0 <= start < end <= 2**64-1:
                    return chrom, start, end
            except ValueError:
                pass
        self.fail('expected CHROM:START-END, e.g. chr1:0-1000000; '
                  'requires 0 <= START < END <= 18446744073709551615', param, ctx)


_REGION = _RegionType()


def _bin_size(ctx, param, value):
    if value is None:
        return None
    try:
        return _parse_bin_size(value)
    except ValueError as exc:
        raise click.BadParameter(str(exc), ctx=ctx, param=param) from None


def _output_options(*, provenance=False):
    def decorate(function):
        options = [
            click.option('-o', '--output', required=True, metavar='PATH', help='New output path; must not already exist.'),
            click.option('--chunk-size', type=_POSITIVE, metavar='INTEGER', default=1_000_000, show_default=True,
                         help='Records per PQS shard or pairs2cool sort run.'),
            click.option('--batch-rows', type=_BATCH_ROWS, metavar='INTEGER', default=65_536, show_default=True,
                         help='Target batch rows.'),
            click.option('--progress/--no-progress', default=None,
                         help='Show stages and elapsed time on stderr (default: terminal only).'),
        ]
        if provenance:
            options.append(click.option('--no-provenance', is_flag=True,
                                        help='Omit operation provenance sidecars.'))
        for option in reversed(options):
            function = option(function)
        return function
    return decorate


def _run(operation, *, show_report=True, render_report=None, show_query_stats=False, **options):
    from .progress import display
    try:
        with display(options.pop('progress', None), getattr(operation, '__name__', 'pqsio')):
            result = operation(**options)
        if render_report is not None:
            render_report(result.to_dict())
        elif show_report:
            click.echo(json.dumps(result.to_dict(), ensure_ascii=False, indent=2))
        if show_query_stats:
            click.echo(json.dumps(result.to_dict()['query_stats'], ensure_ascii=False), err=True)
        status = {'valid': 0, 'invalid': 1, 'incomplete': 3}[result.status] if operation is validate else 0
    except BrokenPipeError:
        click.get_current_context().exit(0)
    except ValueError as exc:
        raise click.UsageError(str(exc)) from None
    except (OSError, RuntimeError) as exc:
        raise click.ClickException(str(exc)) from None
    except KeyboardInterrupt:
        click.echo('pqsio: interrupted', err=True)
        click.get_current_context().exit(130)
    click.get_current_context().exit(status)


@click.group(context_settings=_CONTEXT, invoke_without_command=True)
@click.version_option(__version__, prog_name='pqsio', message='%(prog)s %(version)s')
@click.pass_context
def cli(ctx):
    """Convert, inspect and manage PQS datasets with native Rust operations."""
    if ctx.invoked_subcommand is None:
        click.echo(ctx.get_help())


@cli.command('convert', context_settings=_CONTEXT)
@click.argument('input')
@_output_options()
@click.option('--mode', type=click.Choice(['concat2pairs', 'bam2pairs', 'bam2concat',
                                         'paf2pairs', 'paf2concat', 'pairs2cool']),
              default='concat2pairs', show_default=True, metavar='MODE',
              help='concat2pairs, bam2pairs, bam2concat, paf2pairs, paf2concat or pairs2cool.')
@click.option('--bin-size', '--binsize', '-bs', callback=_bin_size, metavar='BP',
              help='Cooler bin size in bp, e.g. 10000, 10k, 1m; required for pairs2cool.')
@click.option('--min-mapq', type=_MAPQ, metavar='0-255', default=0, show_default=True, help='Minimum alignment or pair MAPQ.')
@click.option('--min-order', type=_POSITIVE, metavar='INTEGER', help='Minimum retained alignments (default: 2 for pairs, 1 for concat).')
@click.option('--max-order', type=click.IntRange(2, _SIZE_MAX), metavar='INTEGER', help='Exclusive read-order upper bound (default: unlimited).')
@click.option('-t', '--threads', type=_POSITIVE, metavar='INTEGER', default=1, show_default=True,
              help='Native workers per stage; pairs2cool: PQS decoding/binning, sorting and pixel compression.')
@click.option('--contigsizes', help='Reference sizes/FAI for PAF or pairs text.')
@click.option('--include-secondary', is_flag=True, help='Retain secondary alignments in BAM/PAF imports.')
@click.option('--samtools', hidden=True)
@click.option('--tmpdir', help='Import/pairs2cool scratch directory (default: output parent).')
@click.option('--pair-position', type=click.Choice(['five-prime', 'leftmost']),
              help='bam2pairs/paf2pairs coordinates (default: leftmost).')
def convert_command(**options):
    """Convert PQS, BAM, PAF and pairs to PQS or Cooler.

    INPUT is a PQS directory, BAM, PAF or pairs text file (optionally gzip/mgzip).
    Both BAM modes accept Hi-C and long-read alignments.
    """
    _run(convert, **options)


def _register_conversion(mode):
    # Reuse the tested conversion options and native dispatch, exposing only
    # options meaningful for this mode. Copy parameters to avoid shared mutation.
    names = {'input', 'output', 'chunk_size', 'batch_rows', 'progress', 'min_mapq', 'threads'}
    if mode == 'pairs2cool':
        names.update(('bin_size', 'contigsizes', 'tmpdir'))
    else:
        names.update(('min_order', 'max_order'))
        if mode != 'concat2pairs':
            names.update(('include_secondary', 'tmpdir'))
            if mode.startswith('paf'):
                names.add('contigsizes')
            else:
                names.add('samtools')
            if mode.endswith('2pairs'):
                names.add('pair_position')
    params = [copy.deepcopy(p) for p in convert_command.params if p.name in names]
    for param in params:
        if param.name == 'bin_size':
            param.required = True

    def run(**options):
        _run(convert, mode=mode, **options)

    source, target = mode.split('2')
    command = click.RichCommand(mode, callback=run, params=params, context_settings=_CONTEXT,
                                help=f'Convert {source.upper()} to {target.upper()} using Rust. Equivalent to convert --mode {mode}.')
    cli.add_command(command)


for _mode in ('bam2pairs', 'bam2concat', 'paf2pairs', 'paf2concat', 'concat2pairs', 'pairs2cool'):
    _register_conversion(_mode)


@cli.command('inspect', context_settings=_CONTEXT)
@click.argument('path')
@click.option('--progress/--no-progress', default=None, help='Show elapsed time on stderr (default: terminal only).')
def inspect_command(path, progress):
    """Show PQS metadata and Parquet footer information."""
    _run(inspect, path=path, progress=progress)


@cli.command('validate', context_settings=_CONTEXT)
@click.argument('path')
@click.option('--level', type=click.Choice(['quick', 'full']), default='quick', show_default=True,
              help='Quick checks footers; full also decodes records.')
@click.option('--max-issues', type=click.IntRange(1, _SIZE_MAX), metavar='INTEGER', default=100, show_default=True,
              help='Maximum stored issue examples.')
@click.option('--progress/--no-progress', default=None, help='Show elapsed time on stderr (default: terminal only).')
def validate_command(**options):
    """Check a PQS dataset and emit a JSON diagnostic report."""
    _run(validate, **options)


@cli.command('subset', context_settings=_CONTEXT)
@click.argument('input')
@_output_options(provenance=True)
@click.option('--min-mapq', type=_MAPQ, metavar='0-255', help='Minimum MAPQ (default: no filter).')
@click.option('--chrom', 'chroms', multiple=True, metavar='CONTIG', help='Select a contig; repeat to select several.')
@click.option('--region', 'regions', multiple=True,
              type=_REGION,
              metavar='CHROM:START-END', help='0-based half-open interval; repeat to select several.')
@click.option('--read-id', 'read_ids', multiple=True, metavar='ID', help='Pairs string read ID; repeat to select several.')
@click.option('--read-index', 'read_indices', multiple=True, type=click.IntRange(0, 2**64-1), metavar='ID',
              help='Concat logical integer read ID; repeat to select several.')
@click.option('--pairs-mode', type=click.Choice(['either', 'both']), help='Pairs endpoint matching (default: either).')
@click.option('--mode', type=click.Choice(['matching-alignments', 'complete-reads']),
              help='Concat selection mode (default: matching-alignments).')
def subset_command(**options):
    """Filter a PQS dataset, preserving its format."""
    indices = options.pop('read_indices')
    if indices and options['read_ids']:
        raise click.UsageError('--read-id and --read-index are mutually exclusive')
    options['read_ids'] = list(indices or options['read_ids']) or None
    for name in ['chroms', 'regions']:
        options[name] = list(options[name]) or None
    if options['mode'] is not None:
        options['mode'] = options['mode'].replace('-', '_')
    options['provenance'] = not options.pop('no_provenance')
    _run(subset, **options)


@cli.command('merge', context_settings=_CONTEXT)
@click.argument('inputs', nargs=-1, required=True)
@_output_options(provenance=True)
def merge_command(**options):
    """Merge same-format PQS datasets in input order."""
    options['inputs'] = list(options['inputs'])
    options['provenance'] = not options.pop('no_provenance')
    _run(merge, **options)


@cli.command('info', context_settings=_CONTEXT)
@click.argument('path')
@click.option('--json', 'as_json', is_flag=True, help='Print machine-readable JSON instead of a table.')
@click.option('--stats', is_flag=True, help='Scan q0 for exact MAPQ distribution and concat read count.')
@click.option('--progress/--no-progress', default=None, help='Show stages on stderr (default: terminal only).')
def info_command(path, as_json, stats, progress):
    """Summarize a PQS dataset; record scanning is opt-in."""
    if as_json:
        _run(info, path=path, stats=stats, progress=progress)
    from rich.console import Console
    from rich.table import Table
    from rich.text import Text
    from .progress import display
    try:
        with display(progress, 'info'):
            data = info(path, stats=stats).to_dict()
        table = Table(title='PQS summary', show_header=False)
        table.add_column('Field', style='cyan')
        table.add_column('Value')
        for label, value in [
            ('Path', data['path']), ('Format', data['format']+' '+data['format_version']),
            ('Coordinates', data['coordinates']), ('Chromosomes', f"{data['nchroms']:,}"),
            ('q0 records', f"{data['q0_records']:,}"), ('q1 records (MAPQ > 0)', f"{data['q1_records']:,}"),
            ('Parquet shards', f"{data['shards']:,}"), ('Parquet size', f"{data['parquet_bytes']:,} bytes"),
        ]:
            table.add_row(label, Text(str(value)))
        declared = data['declared_counts']
        if data['format'] == 'concat':
            reads = data.get('scanned_reads', declared.get('q0_concats', 'unknown'))
            table.add_row('Logical reads' if stats else 'Logical reads (declared)', str(reads))
        if stats:
            hist = data['mapq_histogram']
            for label, low, high in [('MAPQ 0', 0, 1), ('MAPQ 1–9', 1, 10),
                                     ('MAPQ 10–19', 10, 20), ('MAPQ 20–29', 20, 30), ('MAPQ ≥30', 30, 256)]:
                table.add_row(label, f'{sum(hist[low:high]):,}')
        Console().print(table)
    except ValueError as exc:
        raise click.UsageError(str(exc)) from None
    except (OSError, RuntimeError) as exc:
        raise click.ClickException(str(exc)) from None


def _browse_options(function):
    for option in reversed([
        click.option('--columns', help='Comma-separated output columns, in the requested order.'),
        click.option('--min-mapq', type=_MAPQ, default=0, show_default=True),
        click.option('--region', 'regions', multiple=True,
                     type=_REGION,
                     metavar='CHROM:START-END', help='0-based half-open interval; repeat to combine.'),
        click.option('--pairs-mode', type=click.Choice(['either', 'both']), default='either', show_default=True),
        click.option('--no-header', is_flag=True, help='Omit text headers.'),
        click.option('--progress/--no-progress', default=None, help='Show stages on stderr (default: terminal only).'),
    ]):
        function = option(function)
    return function


def _browse_values(options):
    columns = options['columns']
    options['columns'] = None if columns is None else [v.strip() for v in columns.split(',')]
    options['regions'] = list(options['regions'])
    options['header'] = not options.pop('no_header')
    return options


@cli.command('head', context_settings=_CONTEXT)
@click.argument('input')
@click.option('-n', '--limit', type=click.IntRange(0, 2**64-1), default=10, show_default=True)
@_browse_options
def head_command(**options):
    """Preview the first matching rows as TSV (default: 10 rows)."""
    _run(view, show_report=False, **_browse_values(options))


@cli.command('view', context_settings=_CONTEXT)
@click.argument('input')
@click.option('-n', '--limit', type=click.IntRange(0, 2**64-1), default=100, show_default=True)
@click.option('--all', 'all_rows', is_flag=True, help='Stream all matching rows instead of a preview.')
@_browse_options
def view_command(**options):
    """View selected PQS columns/regions as TSV."""
    if options.pop('all_rows'):
        options['limit'] = None
    _run(view, show_report=False, **_browse_values(options))


@cli.command('export', context_settings=_CONTEXT)
@click.argument('input')
@click.option('-o', '--output', required=True, help='New text file (.gz/.mgz compresses), or - for stdout.')
@click.option('--format', type=click.Choice(['auto', 'pairs', 'concat', 'tsv']), default='auto', show_default=True)
@click.option('-n', '--limit', type=click.IntRange(0, 2**64-1), help='Maximum exported rows (default: all).')
@click.option('-t', '--threads', type=_POSITIVE, default=1, show_default=True, help='Compression workers.')
@_browse_options
def export_command(**options):
    """Export pairs/concat PQS to standard pairs, concat or TSV using Rust I/O."""
    _run(export, show_report=options['output'] != '-', **_browse_values(options))


def _stats_table(data):
    from rich.console import Console
    from rich.table import Table
    from rich.text import Text
    table = Table(title='PQS quality statistics', show_header=False)
    table.add_column('Metric', style='cyan')
    table.add_column('Value')
    for key in ('format', 'source_quality', 'min_mapq', 'scanned_records', 'selected_records',
                'filtered_records', 'selected_fraction', 'mapq_zero_fraction', 'mapq_ge30_fraction',
                'cis_records', 'trans_records', 'cis_fraction', 'trans_fraction', 'scanned_reads',
                'selected_reads', 'mean_read_order', 'mean_read_length', 'multi_contig_reads',
                'mean_identity', 'aligned_reference_bases', 'nonfinite_identity_records'):
        if key in data:
            value = data[key]
            shown = 'N/A' if value is None else f'{value:.4f}' if isinstance(value, float) else str(value)
            table.add_row(key.replace('_', ' '), Text(shown))
    hist = data['mapq_histogram']
    for label, lo, hi in [('MAPQ 0', 0, 1), ('MAPQ 1–9', 1, 10), ('MAPQ 10–19', 10, 20),
                           ('MAPQ 20–29', 20, 30), ('MAPQ ≥30', 30, 256)]:
        table.add_row(label, str(sum(hist[lo:hi])))
    for key in ('cis_distance_bp', 'strand_pairs', 'read_order_histogram', 'filter_reasons'):
        for label, value in data.get(key, {}).items():
            table.add_row(Text(key.replace('_', ' ') + ': ' + label), str(value))
    Console().print(table)


@cli.command('stats', context_settings=_CONTEXT)
@click.argument('path')
@click.option('--min-mapq', type=_MAPQ, default=0, show_default=True)
@click.option('--json', 'as_json', is_flag=True, help='Print complete machine-readable metrics.')
@click.option('--progress/--no-progress', default=None)
def stats_command(path, min_mapq, as_json, progress):
    """Scan q0 for pairs/concat quality metrics without counting q1 twice."""
    _run(stats, path=path, min_mapq=min_mapq, progress=progress,
         render_report=None if as_json else _stats_table)


@cli.command('query', context_settings=_CONTEXT)
@click.argument('input')
@click.option('-o', '--output', default='-', show_default=True, help='Text output; - streams to stdout.')
@click.option('--format', type=click.Choice(['auto', 'pairs', 'concat', 'tsv']), default='tsv', show_default=True)
@click.option('--index', type=click.Choice(['auto', 'off', 'require']), default='auto', show_default=True)
@click.option('--build-index/--no-build-index', 'auto_index', default=True, show_default=True,
              help='With --index auto, build a missing index for the selected partition.')
@click.option('--mode', type=click.Choice(['matching-alignments', 'complete-reads']),
              help='Concat selection; complete-reads also returns out-of-region/low-MAPQ alignments.')
@click.option('--show-stats', is_flag=True, help='Print native query diagnostics to stderr.')
@click.option('-n', '--limit', type=click.IntRange(0, 2**64-1), help='Maximum rows (default: all; can split reads).')
@click.option('-t', '--threads', type=_POSITIVE, default=1, show_default=True, help='Output compression workers.')
@_browse_options
def query_command(**options):
    """Query the union of one or more --region CHROM:START-END intervals.

    Regions are 0-based half-open. Output coordinates are preserved.
    """
    if not options['regions']:
        raise click.UsageError('query requires at least one --region CHROM:START-END')
    mode = options.pop('mode')
    options['filter_mode'] = None if mode is None else mode.replace('-', '_')
    show_stats = options.pop('show_stats')
    _run(export, show_report=options['output'] != '-', show_query_stats=show_stats,
         **_browse_values(options))


@cli.group('index', context_settings=_CONTEXT, invoke_without_command=True)
@click.pass_context
def index_command(ctx):
    """Build, check or rebuild independent q0/q1 region indexes."""
    if ctx.invoked_subcommand is None:
        click.echo(ctx.get_help())


def _index_operation(path, quality, rebuild=None):
    qualities = ('q0', 'q1') if quality == 'both' else (quality,)
    results = []
    for selected in qualities:
        if rebuild is not None:
            build_index(path, quality=selected, rebuild=rebuild)
        results.append(index_status(path, quality=selected).to_dict())
    return ConvertResult({'path': str(path), 'indexes': results})


def _index_options(function):
    function = click.option('--progress/--no-progress', default=None)(function)
    function = click.option('--quality', type=click.Choice(['q0', 'q1', 'both']),
                            default='both', show_default=True)(function)
    return click.argument('path')(function)


@index_command.command('build', context_settings=_CONTEXT)
@_index_options
@click.option('--rebuild', is_flag=True, help='Replace CURRENT even if an index already exists.')
def index_build_command(**options):
    """Decode selected partitions and publish indexes; source PQS is unchanged."""
    _run(_index_operation, **options)


@index_command.command('status', context_settings=_CONTEXT)
@_index_options
def index_status_command(**options):
    """Check source identity and index checksums; print valid/missing/invalid JSON."""
    _run(_index_operation, **options)


@index_command.command('rebuild', context_settings=_CONTEXT)
@_index_options
def index_rebuild_command(**options):
    """Rebuild selected indexes, retaining older generations for active readers."""
    _run(_index_operation, rebuild=True, **options)


def main(argv=None):
    """Run the rich CLI, returning a status for both installed and module entry points."""
    try:
        cli.main(args=argv, prog_name='pqsio')
    except SystemExit as exc:
        return exc.code
    except KeyboardInterrupt:
        click.echo('pqsio: interrupted', err=True)
        return 130
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
