#!/usr/bin/env python3
"""Render documentation figures from measured format benchmarks.

Requires matplotlib only when regenerating figures, not when building docs.
By default, read the published CSV. --report refreshes that CSV from a full
format_bench.py JSON report, without publishing source names or local paths.

Figure contract: a quantitative grid shows the speedup of PQS relative to text
and gzip at every measured scale; a separate grid shows resource tradeoffs at
one million records. Text uses uncompressed PQS as its denominator; gzip uses
default PQS. All ratios use comparator / PQS, with a 1x equality line.
Timings and RSS are medians of technical repeats, not biological replicates.
No uncertainty intervals or significance claims are shown. Source CSV retains
the observed timing range and repeat count. Export editable SVG for the website
and optional PNG previews at the same dimensions; no journal submission target.
"""
import argparse
import csv
import json
import math
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
ASSETS = ROOT / 'docs' / 'assets' / 'benchmarks'
FIELDS = ('kind', 'records', 'operation', 'storage', 'repetitions', 'threads',
          'median_seconds', 'min_seconds', 'max_seconds',
          'median_peak_rss_mib', 'disk_bytes')
KINDS = ('pairs', 'concat')
FORMATS = ('text', 'gzip', 'pqs_uncompressed', 'pqs_default')
BASELINES = {'text': 'pqs_uncompressed', 'gzip': 'pqs_default'}
COMPARISONS = {'text': 'Text / PQS uncompressed', 'gzip': 'Gzip / PQS default'}
OPERATIONS = ('read_all', 'mapq_count')
COLORS = {'text': '#5574B9', 'gzip': '#218779'}


def export_summary(report_path, summary_path, operations=OPERATIONS):
    report = json.loads(report_path.read_text())
    if report.get('benchmark_version') != 2 or tuple(report['variants']) != FORMATS:
        raise ValueError('Expected the four-variant compression benchmark; rerun the benchmark scripts')
    summary_path.parent.mkdir(parents=True, exist_ok=True)
    with summary_path.open('w', newline='') as stream:
        writer = csv.DictWriter(stream, fieldnames=FIELDS, lineterminator='\n')
        writer.writeheader()
        for case in report['cases']:
            for operation in operations:
                for storage in FORMATS:
                    stats = case['summary'][operation][storage]
                    writer.writerow(dict(
                        kind=case['kind'], records=case['rows'],
                        operation=operation, storage=storage,
                        repetitions=len(case['samples'][operation][storage]),
                        threads=report['threads'],
                        disk_bytes=case['bytes'][storage], **stats,
                    ))


def load_summary(path, operations=OPERATIONS):
    with path.open(newline='') as stream:
        rows = list(csv.DictReader(stream))
    measurements = {}
    for row in rows:
        key = (row['kind'], int(row['records']), row['operation'], row['storage'])
        if key in measurements:
            raise ValueError(f'Duplicate measurement: {key}')
        measurements[key] = {name: float(row[name]) for name in FIELDS[4:]}
        values = measurements[key]
        if any(not math.isfinite(v) or v <= 0 for v in values.values()):
            raise ValueError(f'Expected finite positive measurements: {key}')
    for kind in KINDS:
        scales = sorted({key[1] for key in measurements if key[0] == kind})
        if not scales or 1000000 not in scales:
            raise ValueError(f'{kind}: expected measured scales including 1,000,000 records')
        for records in scales:
            for operation in operations:
                for storage in FORMATS:
                    key = (kind, records, operation, storage)
                    if key not in measurements:
                        raise ValueError(f'Missing measurement: {key}')
    return measurements


def draw_panel(ax, groups, ticks, limit, equality):
    """Draw paired comparisons with consistent order and exact ratio labels."""
    for index, (label, ratios) in enumerate(groups):
        for offset, storage in ((-0.19, 'text'), (0.19, 'gzip')):
            ratio = ratios[storage]
            y = index + offset
            ax.barh(y, ratio, height=0.31, color=COLORS[storage], zorder=3)
            ax.annotate(f'{ratio:.2f}×', (ratio, y), xytext=(6, 0),
                        textcoords='offset points', va='center', fontsize=11,
                        fontweight='bold', color='#222B38', zorder=5,
                        bbox=dict(facecolor='white', edgecolor='none', pad=0.2))
    ax.axvline(1, color='#77808D', linewidth=1, linestyle=(0, (3, 3)), zorder=4)
    if equality is not None:
        ax.annotate(f'1× = {equality}', xy=(1, 1), xycoords=('data', 'axes fraction'),
                    xytext=(6, 8), textcoords='offset points', fontsize=10,
                    color='#525D6B', va='bottom',
                    arrowprops=dict(arrowstyle='-', color='#77808D', linewidth=0.8))
    ax.set_yticks(range(len(groups)), [label for label, _ in groups])
    ax.set_ylim(len(groups) - 0.5, -0.5)
    ax.set_xlim(0, limit)
    ax.set_xticks(ticks, [f'{tick:g}×' for tick in ticks])
    ax.grid(axis='x', color='#E7EBF0', linewidth=0.7, zorder=0)
    ax.tick_params(axis='both', length=0, pad=8)
    ax.spines[['top', 'right', 'left']].set_visible(False)
    ax.spines['bottom'].set_color('#D4DAE2')


def save_figure(fig, output_dir, name, preview_dir):
    svg_path = output_dir / f'{name}.svg'
    fig.savefig(svg_path, facecolor='white',
                metadata={'Date': None, 'Creator': 'pqsio format benchmark'})
    svg_path.write_text('\n'.join(line.rstrip() for line in svg_path.read_text().splitlines()) + '\n')
    if preview_dir is not None:
        preview_dir.mkdir(parents=True, exist_ok=True)
        fig.savefig(preview_dir / f'{name}.png', dpi=300, facecolor='white')
        fig.savefig(preview_dir / f'{name}.pdf', facecolor='white')


def make_figure(plt, title, subtitle, xlabel, footer, reading_guide):
    from matplotlib.patches import Patch

    fig, axes = plt.subplots(2, 1, figsize=(8.4, 6.8))
    fig.subplots_adjust(left=0.23, right=0.91, top=0.72, bottom=0.15, hspace=0.58)
    fig.suptitle(title, x=0.055, y=0.975, ha='left', fontsize=18, fontweight='bold')
    fig.text(0.055, 0.91, subtitle, fontsize=11, color='#525D6B')
    fig.legend(handles=[Patch(facecolor=COLORS[s], label=COMPARISONS[s])
                        for s in ('text', 'gzip')],
               loc='upper left', bbox_to_anchor=(0.045, 0.885), ncol=2,
               fontsize=11, frameon=False, handlelength=1.5, columnspacing=2.5)
    fig.text(0.055, 0.795, reading_guide, fontsize=11, fontweight='bold', color='#222B38')
    axes[-1].set_xlabel(xlabel, labelpad=12, fontsize=11)
    fig.text(0.055, 0.025, footer, fontsize=10, color='#525D6B')
    return fig, axes


def plot_timings(plt, data, operation, output_dir, preview_dir):
    writing = operation == 'write_all'
    title = {'read_all': 'Read all records and columns',
             'mapq_count': 'Count records with MAPQ ≥ 30',
             'write_all': 'Write all records and columns'}[operation]
    repeats = sorted({int(row['repetitions']) for row in data.values()})
    threads = sorted({int(row['threads']) for row in data.values()})
    run_label = '/'.join(map(str, repeats))
    thread_label = '/'.join(map(str, threads))
    footer = (f'Median of {run_label} runs · prepared input · {thread_label}-CPU budget · close included'
              if writing else
              f'Median of {run_label} measured runs · warm cache · {thread_label} threads')
    subtitle = 'PQS speedup  =  comparison format time / corresponding PQS time'
    fig, axes = make_figure(
        plt, title, subtitle,
        'PQS speedup · higher is faster',
        footer,
        '1× = same speed     >1× = PQS faster     <1× = PQS slower',
    )
    maximum = max(data[(k, n, operation, s)]['median_seconds'] /
                  data[(k, n, operation, BASELINES[s])]['median_seconds']
                  for k, n, op, s in data if op == operation and s in BASELINES)
    from matplotlib.ticker import MaxNLocator
    ticks = MaxNLocator(nbins=5).tick_values(0, max(1.5, maximum * 1.2))
    limit = ticks[-1]
    for ax, kind in zip(axes, KINDS):
        scales = sorted({key[1] for key in data if key[0] == kind})
        groups = []
        for records in scales:
            ratios = {storage: data[(kind, records, operation, storage)]['median_seconds'] /
                      data[(kind, records, operation, BASELINES[storage])]['median_seconds']
                      for storage in ('text', 'gzip')}
            groups.append((f'{records:,} records', ratios))
        draw_panel(ax, groups, ticks, limit, 'same speed')
        ax.set_title(kind.capitalize(), x=-0.255, loc='left', fontsize=13, fontweight='bold', pad=13)
    save_figure(fig, output_dir, f'format-{operation.replace("_", "-")}', preview_dir)
    plt.close(fig)


def plot_resources(plt, data, output_dir, preview_dir, writing=False):
    run_label = '/'.join(map(str, sorted({int(row['repetitions']) for row in data.values()})))
    footer = (f'Peak RSS includes both prepared inputs · medians of {run_label} runs' if writing else
              f'PQS disk includes q0 + q1 + metadata · RSS: median of {run_label} runs')
    fig, axes = make_figure(
        plt, 'Write output size and memory' if writing else 'Disk space and memory',
        'One million records · ratio = comparison format usage / PQS usage',
        'Usage ratio · higher means PQS uses less',
        footer,
        '1× = same usage     >1× = PQS uses less     <1× = PQS uses more',
    )
    metrics = [('Disk space', 'read_all', 'disk_bytes'),
               ('Full-read\npeak memory', 'read_all', 'median_peak_rss_mib'),
               ('MAPQ-count\npeak memory', 'mapq_count', 'median_peak_rss_mib')]
    if writing:
        metrics = [('Disk space', 'write_all', 'disk_bytes'),
                   ('Write process\npeak memory', 'write_all', 'median_peak_rss_mib')]
    maximum = max(data[(kind, 1000000, op, storage)][field] /
                  data[(kind, 1000000, op, BASELINES[storage])][field]
                  for kind in KINDS for _, op, field in metrics for storage in BASELINES)
    from matplotlib.ticker import MaxNLocator
    ticks = MaxNLocator(nbins=5).tick_values(0, max(1.5, maximum * 1.2))
    for ax, kind in zip(axes, KINDS):
        groups = []
        for label, operation, field in metrics:
            ratios = {storage: data[(kind, 1000000, operation, storage)][field] /
                      data[(kind, 1000000, operation, BASELINES[storage])][field]
                      for storage in ('text', 'gzip')}
            groups.append((label, ratios))
        draw_panel(ax, groups, ticks, ticks[-1], 'same usage')
        ax.set_title(kind.capitalize(), x=-0.255, loc='left', fontsize=13, fontweight='bold', pad=13)
    save_figure(fig, output_dir, 'format-write-resources' if writing else 'format-resources', preview_dir)
    plt.close(fig)


def plot_overview(plt, data, write_data, output_dir, preview_dir):
    """One-million-record overview; all scales remain in the detailed figures."""
    from matplotlib.patches import Patch
    from matplotlib.ticker import MaxNLocator

    fig, axes = plt.subplots(3, 1, figsize=(8.4, 7.3))
    fig.subplots_adjust(left=0.20, right=0.91, top=0.76, bottom=0.11, hspace=0.85)
    fig.suptitle('PQS performance at one million records', x=0.055, y=0.975,
                 ha='left', fontsize=17, fontweight='bold')
    fig.legend(handles=[Patch(facecolor=COLORS[s], label=COMPARISONS[s]) for s in BASELINES],
               loc='upper left', bbox_to_anchor=(0.045, 0.94), ncol=2,
               fontsize=11, frameon=False)
    fig.text(0.055, 0.85, 'Speedup = comparison time / PQS time · higher is faster', fontsize=11)
    fig.text(0.055, 0.81, '1× = same speed     >1× = PQS faster     <1× = PQS slower',
             fontsize=10, color='#525D6B')
    for ax, (label, operation, measurements) in zip(axes, [
        ('Read all columns', 'read_all', data),
        ('Count MAPQ ≥ 30', 'mapq_count', data),
        ('Write complete dataset', 'write_all', write_data),
    ]):
        groups = [(kind.capitalize(), {
            ref: measurements[(kind, 1000000, operation, ref)]['median_seconds'] /
                 measurements[(kind, 1000000, operation, baseline)]['median_seconds']
            for ref, baseline in BASELINES.items()}) for kind in KINDS]
        maximum = max(v for _, ratios in groups for v in ratios.values())
        ticks = MaxNLocator(nbins=5).tick_values(0, max(1.5, maximum * 1.25))
        draw_panel(ax, groups, ticks, ticks[-1], None)
        ax.set_title(label, loc='left', x=-0.20, fontsize=12, fontweight='bold', pad=10)
    fig.text(0.055, 0.025, 'Median of 5 runs · 4 CPUs · warm reads / buffered writes\n'
             'PQS writes include q0 + q1 + metadata. Panel scales differ.',
             fontsize=10, color='#525D6B')
    save_figure(fig, output_dir, 'format-overview', preview_dir)
    plt.close(fig)


def update_tables(page, data, write_data):
    """Keep exact values reproducible without mixing the two PQS baselines."""
    if write_data is None:
        raise ValueError('Write measurements are required to update the page tables')
    lines = ['??? info "Exact measurements and ratios"', '',
             '    Times are in milliseconds; disk and peak RSS are in MiB (2²⁰ bytes).',
             '    Ratios are calculated from unrounded medians. Each row names its PQS baseline.', '']
    for title, measurements, operations in (
        ('Read all records and columns', data, ('read_all',)),
        ('Count records with MAPQ ≥ 30', data, ('mapq_count',)),
        ('Write a complete dataset', write_data, ('write_all',)),
    ):
        lines += [f'    **{title}**', '',
                  '    | Dataset | Records | Comparison | Text / Gzip (ms) | PQS (ms) | PQS speedup |',
                  '    | --- | ---: | --- | ---: | ---: | ---: |']
        for kind in KINDS:
            scales = sorted({key[1] for key in measurements if key[0] == kind})
            for records in scales:
                for storage, baseline in BASELINES.items():
                    ref = measurements[(kind, records, operations[0], storage)]['median_seconds']
                    pqs = measurements[(kind, records, operations[0], baseline)]['median_seconds']
                    lines.append(f'    | {kind} | {records:,} | {COMPARISONS[storage]} | '
                                 f'{ref * 1000:.2f} | {pqs * 1000:.2f} | **{ref / pqs:.2f}×** |')
        lines.append('')
    for title, measurements, metrics in (
        ('Storage and read memory at one million records', data,
         [('Disk space', 'read_all', 'disk_bytes', 2**20),
          ('Full-read peak RSS', 'read_all', 'median_peak_rss_mib', 1),
          ('MAPQ-count peak RSS', 'mapq_count', 'median_peak_rss_mib', 1)]),
        ('Write output size and memory at one million records', write_data,
         [('Disk space', 'write_all', 'disk_bytes', 2**20),
          ('Write-process peak RSS', 'write_all', 'median_peak_rss_mib', 1)]),
    ):
        lines += [f'    **{title}**', '',
                  '    | Dataset | Metric | Comparison | Text / Gzip (MiB) | PQS (MiB) | Usage ratio |',
                  '    | --- | --- | --- | ---: | ---: | ---: |']
        for kind in KINDS:
            for label, operation, field, scale in metrics:
                for storage, baseline in BASELINES.items():
                    ref = measurements[(kind, 1000000, operation, storage)][field] / scale
                    pqs = measurements[(kind, 1000000, operation, baseline)][field] / scale
                    lines.append(f'    | {kind} | {label} | {COMPARISONS[storage]} | '
                                 f'{ref:.2f} | {pqs:.2f} | **{ref / pqs:.2f}×** |')
        lines.append('')
    start, end = '<!-- benchmark-tables:start -->', '<!-- benchmark-tables:end -->'
    text = page.read_text()
    if text.count(start) != 1 or text.count(end) != 1:
        raise ValueError('Page must contain exactly one benchmark table marker pair')
    a, b = text.index(start) + len(start), text.index(end)
    if b <= a:
        raise ValueError('Benchmark table markers are out of order')
    page.write_text(text[:a] + '\n\n' + '\n'.join(lines) + '\n' + text[b:])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--report', type=Path, help='Refresh CSV from a measured JSON report')
    parser.add_argument('--summary', type=Path, default=ASSETS / 'format-summary.csv')
    parser.add_argument('--write-report', type=Path, help='Refresh write CSV from a measured JSON report')
    parser.add_argument('--write-summary', type=Path, default=ASSETS / 'format-write-summary.csv')
    parser.add_argument('--output-dir', type=Path, default=ASSETS)
    parser.add_argument('--preview-dir', type=Path, help='Optional directory for PNG previews')
    parser.add_argument('--update-page', type=Path, help='Refresh marked exact-value tables in a Markdown page')
    args = parser.parse_args()
    if args.report:
        export_summary(args.report, args.summary)
    data = load_summary(args.summary)
    if args.write_report:
        export_summary(args.write_report, args.write_summary, ('write_all',))
    write_data = (load_summary(args.write_summary, ('write_all',))
                  if args.write_summary.exists() else None)
    if args.update_page:
        update_tables(args.update_page, data, write_data)

    import matplotlib as mpl
    mpl.use('Agg')
    import matplotlib.pyplot as plt
    mpl.rcParams.update({
        'font.family': 'sans-serif', 'font.sans-serif': ['DejaVu Sans'],
        'font.size': 11, 'text.color': '#222B38', 'axes.labelcolor': '#222B38',
        'xtick.color': '#525D6B', 'ytick.color': '#222B38',
        'svg.fonttype': 'none', 'svg.hashsalt': 'pqsio-format-benchmark',
        'pdf.fonttype': 42, 'axes.linewidth': 0.8,
    })
    args.output_dir.mkdir(parents=True, exist_ok=True)
    for operation in OPERATIONS:
        plot_timings(plt, data, operation, args.output_dir, args.preview_dir)
    plot_resources(plt, data, args.output_dir, args.preview_dir)
    if write_data:
        plot_overview(plt, data, write_data, args.output_dir, args.preview_dir)
        plot_timings(plt, write_data, 'write_all', args.output_dir, args.preview_dir)
        plot_resources(plt, write_data, args.output_dir, args.preview_dir, writing=True)
    print(f'Rendered {6 if write_data else 3} figures from {len(data) + len(write_data or {})} measured summaries.')


if __name__ == '__main__':
    main()
