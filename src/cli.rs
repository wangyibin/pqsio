use anyhow::{bail, ensure, Context, Result};
use clap::builder::{
    styling::{AnsiColor, Effects},
    Styles,
};
use clap::{Arg, ArgAction, ArgMatches, Command};
use pqsio::{metadata::Value, *};
use std::io::{self, Write};
use std::path::Path;

// Match cphasing-rs/src/cli.rs terminal help palette.
const STYLES: Styles = Styles::styled()
    .header(AnsiColor::Green.on_default().effects(Effects::BOLD))
    .usage(AnsiColor::Green.on_default().effects(Effects::BOLD))
    .literal(AnsiColor::Cyan.on_default().effects(Effects::BOLD))
    .placeholder(AnsiColor::Yellow.on_default());

fn styled(command: Command) -> Command {
    command
        .styles(STYLES)
        .color(if std::env::var_os("NO_COLOR").is_some() {
            clap::ColorChoice::Never
        } else {
            clap::ColorChoice::Auto
        })
}

const MODES: [&str; 6] = [
    "concat2pairs",
    "bam2pairs",
    "bam2concat",
    "paf2pairs",
    "paf2concat",
    "pairs2cool",
];
fn option(name: &'static str, help: &'static str) -> Arg {
    Arg::new(name).long(name).help(help)
}
fn flag(name: &'static str, help: &'static str) -> Arg {
    option(name, help).action(ArgAction::SetTrue)
}
fn input() -> Arg {
    Arg::new("input").required(true)
}
fn output(required: bool) -> Arg {
    option("output", "New output path; parent must exist")
        .short('o')
        .required(required)
}
fn command(name: &'static str, about: &'static str) -> Command {
    styled(Command::new(name))
        .about(about)
        .disable_help_subcommand(true)
        .arg(flag("progress", "Show stages on stderr").overrides_with("no-progress"))
        .arg(
            flag("no-progress", "Disable progress (default: terminal only)")
                .overrides_with("progress"),
        )
}
fn sizing(c: Command) -> Command {
    c.arg(
        option("chunk-size", "Records per shard/sorting run")
            .default_value("1000000")
            .help_heading("Performance"),
    )
    .arg(
        option("batch-rows", "Target rows per batch")
            .default_value("65536")
            .help_heading("Performance"),
    )
}
fn mapq() -> Arg {
    option("min-mapq", "Minimum MAPQ (0-255)")
        .default_value("0")
        .help_heading("Filtering")
}
fn threads() -> Arg {
    option("threads", "Workers per native stage")
        .short('t')
        .default_value("1")
        .help_heading("Performance")
}
fn regions() -> Arg {
    option(
        "region",
        "0-based half-open CHROM:START-END; repeat for union",
    )
    .value_name("CHROM:START-END")
    .action(ArgAction::Append)
}
fn pairs_mode() -> Arg {
    option("pairs-mode", "Endpoint matching").value_parser(["either", "both"])
}
fn browse(c: Command) -> Command {
    c.arg(regions())
        .arg(mapq())
        .arg(pairs_mode().default_value("either"))
        .arg(option("columns", "Comma-separated output columns"))
        .arg(flag("no-header", "Omit text header"))
        .arg(option("limit", "Maximum rows; may split concat reads").short('n'))
}
fn format_arg(default: &'static str) -> Arg {
    option("format", "Output text format")
        .value_parser(["auto", "pairs", "concat", "tsv"])
        .default_value(default)
}
fn conversion(mode: &'static str) -> Command {
    let all = mode == "convert";
    let about = match mode {
        "bam2concat" => "Convert BAM alignments to concat PQS",
        "bam2pairs" => "Convert BAM alignments to pairs PQS",
        "concat2pairs" => "Expand concat PQS reads into pairwise contacts in pairs PQS",
        "paf2concat" => "Convert PAF alignments to concat PQS",
        "paf2pairs" => "Convert PAF alignments to pairs PQS",
        "pairs2cool" => "Bin pairs PQS or pairs text into a Cooler contact matrix (.cool)",
        _ => "Convert files with --mode (default: concat2pairs)",
    };
    let mut c = sizing(command(mode, about))
        .arg(input())
        .arg(output(true))
        .arg(mapq())
        .arg(threads());
    if all {
        c = c.arg(
            option("mode", "Conversion mode")
                .value_parser(MODES)
                .default_value("concat2pairs"),
        );
    }
    if all || mode != "pairs2cool" {
        c = c
            .arg(option("min-order", "Minimum retained alignment count").help_heading("Filtering"))
            .arg(
                option("max-order", "Exclusive retained alignment upper bound")
                    .help_heading("Filtering"),
            );
    }
    if all || mode == "pairs2cool" {
        c = c.arg(
            option("bin-size", "Bin size: 10000, 10k, 1m, 1.5m")
                .alias("binsize")
                .required(!all)
                .help_heading("Cooler output"),
        );
    }
    if all || mode != "concat2pairs" {
        c = c.arg(option("tmpdir", "Existing scratch directory").help_heading("Performance"));
    }
    if all || mode.starts_with("paf") || mode == "pairs2cool" {
        c = c.arg(option("contigsizes", "Reference sizes/FAI").help_heading("Alignment input"));
    }
    if all || mode.starts_with("bam") || mode.starts_with("paf") {
        c = c.arg(
            flag("include-secondary", "Include secondary alignments")
                .help_heading("Alignment input"),
        );
    }
    if all || mode == "bam2pairs" || mode == "paf2pairs" {
        c = c.arg(
            option("pair-position", "Pair coordinate selection")
                .value_parser(["five-prime", "leftmost"])
                .help_heading("Alignment input"),
        );
    }
    if all || mode.starts_with("bam") {
        c = c.arg(option("samtools", "Legacy ignored option; BAM decoding is native").hide(true));
    }
    c
}
fn app() -> Command {
    let mut c = styled(Command::new("pqsio"))
        .version(env!("CARGO_PKG_VERSION"))
        .about("Native Rust PQS conversions, browsing and dataset operations")
        .disable_help_subcommand(true);
    for mode in MODES.into_iter().chain(["convert"]) {
        c = c.subcommand(conversion(mode).display_order(0));
    }
    c = c
        .subcommand(command("inspect", "Inspect PQS metadata and Parquet footers as JSON").arg(input()))
        .subcommand(
            command("validate", "Check PQS integrity and report validation results as JSON")
                .arg(input())
                .arg(
                    option("level", "Validation depth")
                        .value_parser(["quick", "full"])
                        .default_value("quick"),
                )
                .arg(option("max-issues", "Maximum stored examples").default_value("100")),
        )
        .subcommand(
            command("info", "Summarize PQS format, contigs and record counts")
                .arg(input())
                .arg(flag("json", "Print JSON"))
                .arg(flag("stats", "Scan MAPQ statistics")),
        )
        .subcommand(
            command("stats", "Compute PQS mapping-quality and contact statistics")
                .arg(input())
                .arg(mapq())
                .arg(flag("json", "Print JSON")),
        );
    for (name, about) in [
        ("head", "Preview the first PQS records (default: 10 rows)"),
        ("view", "Preview PQS records with filters and selected columns (default: 100 rows)"),
        ("export", "Export PQS records to pairs, concat or TSV text, optionally gzip-compressed"),
        ("query", "Query PQS records by genomic region using indexes when available"),
    ] {
        let mut cmd = browse(command(name, about)).arg(input());
        if name == "head" || name == "view" {
            cmd = cmd.mut_arg("limit", |arg| {
                arg.default_value(if name == "head" { "10" } else { "100" })
            });
        }
        if name == "view" {
            cmd = cmd.arg(flag("all", "Stream all matching rows"));
        }
        if name == "export" || name == "query" {
            cmd = cmd
                .arg(output(name == "export"))
                .arg(threads())
                .arg(format_arg(if name == "query" { "tsv" } else { "auto" }));
        }
        if name == "query" {
            cmd = cmd
                .arg(
                    option("index", "Region index policy")
                        .value_parser(["auto", "off", "require"])
                        .default_value("auto"),
                )
                .arg(
                    flag(
                        "build-index",
                        "Automatically build missing indexes (default)",
                    )
                    .overrides_with("no-build-index"),
                )
                .arg(
                    flag("no-build-index", "Scan if index is missing")
                        .overrides_with("build-index"),
                )
                .arg(
                    option("mode", "Concat read selection")
                        .value_parser(["matching-alignments", "complete-reads"]),
                )
                .arg(flag("show-stats", "Query diagnostics as JSON on stderr"));
        }
        c = c.subcommand(cmd);
    }
    c = c
        .subcommand(
            sizing(command("subset", "Save filtered records as a new PQS dataset"))
                .arg(input())
                .arg(output(true))
                .arg(option("min-mapq", "Minimum MAPQ"))
                .arg(option("chrom", "Select contig; repeat to combine").action(ArgAction::Append))
                .arg(regions())
                .arg(pairs_mode())
                .arg(
                    option("read-id", "Pairs string read ID")
                        .action(ArgAction::Append)
                        .conflicts_with("read-index"),
                )
                .arg(option("read-index", "Concat logical read ID").action(ArgAction::Append))
                .arg(
                    option("mode", "Concat selection")
                        .value_parser(["matching-alignments", "complete-reads"]),
                )
                .arg(flag("no-provenance", "Omit provenance")),
        )
        .subcommand(
            sizing(command(
                "merge",
                "Merge PQS datasets of the same format into a new dataset",
            ))
            .arg(input().num_args(1..))
            .arg(output(true))
            .arg(flag("no-provenance", "Omit provenance")),
        );
    let mut index = styled(Command::new("index"))
        .about("Build, check or rebuild genomic region indexes for q0/q1 partitions")
        .disable_help_subcommand(true);
    for (name, about) in [
        ("build", "Build genomic region indexes for selected quality partitions"),
        ("status", "Show index availability and validity for selected quality partitions"),
        ("rebuild", "Replace genomic region indexes for selected quality partitions"),
    ] {
        let mut cmd = command(name, about)
            .arg(input())
            .arg(
                option("quality", "Partition")
                    .value_parser(["q0", "q1", "both"])
                    .default_value("both"),
            );
        if name == "build" {
            cmd = cmd.arg(flag("rebuild", "Replace an existing index"));
        }
        index = index.subcommand(cmd);
    }
    c.subcommand(index)
}
fn get<'a>(m: &'a ArgMatches, key: &str) -> Option<&'a str> {
    m.try_get_one::<String>(key)
        .ok()
        .flatten()
        .map(String::as_str)
}
fn on(m: &ArgMatches, key: &str) -> bool {
    m.try_get_one::<bool>(key)
        .ok()
        .flatten()
        .copied()
        .unwrap_or(false)
}
fn list(m: &ArgMatches, key: &str) -> Vec<String> {
    m.try_get_many::<String>(key)
        .ok()
        .flatten()
        .map(|v| v.cloned().collect())
        .unwrap_or_default()
}
fn num(m: &ArgMatches, key: &str, default: u64) -> u64 {
    get(m, key)
        .map(|v| v.parse().expect("validated number"))
        .unwrap_or(default)
}
fn path(m: &ArgMatches, key: &str) -> Option<std::path::PathBuf> {
    get(m, key).map(Into::into)
}
fn region(s: &str) -> Result<Region> {
    let error = "expected CHROM:START-END, e.g. chr1:0-1000000; requires 0 <= START < END <= UInt64 maximum";
    let (contig, span) = s.rsplit_once(':').context(error)?;
    let (a, b) = span.split_once('-').context(error)?;
    ensure!(
        !contig.is_empty()
            && !a.is_empty()
            && !b.is_empty()
            && a.bytes().chain(b.bytes()).all(|c| c.is_ascii_digit()),
        "{error}"
    );
    let start = a.parse::<u64>().context(error)?;
    let end = b.parse::<u64>().context(error)?;
    ensure!(start < end, "{error}");
    Ok(Region {
        contig: contig.into(),
        start,
        end,
    })
}
// Exact decimal scaling/truncation, including exponent notation; no floating-point rounding.
fn bin_size(raw: &str) -> Result<u64> {
    let error = "bin_size must be positive bp in 1..=9223372036854775807 (e.g. 10k, 1m, 1.5m)";
    let mut text = raw.trim().to_ascii_lowercase().replace(' ', "");
    let unit = match text.as_bytes().last() {
        Some(b'k') => 3,
        Some(b'm') => 6,
        Some(b'g') => 9,
        _ => 0,
    };
    if unit != 0 {
        text.pop();
    }
    let (mantissa, exponent) = match text.split_once('e') {
        Some((m, e)) => (m, e.parse::<i64>().context(error)?),
        None => (text.as_str(), 0),
    };
    let mantissa = mantissa.strip_prefix('+').unwrap_or(mantissa);
    let mut digits = String::new();
    let mut fraction = None;
    for ch in mantissa.bytes() {
        if ch == b'.' && fraction.is_none() {
            fraction = Some(0i64);
        } else if ch.is_ascii_digit() {
            digits.push(ch as char);
            if let Some(n) = fraction.as_mut() {
                *n += 1;
            }
        } else {
            bail!("{error}");
        }
    }
    let digits = digits.trim_start_matches('0');
    ensure!(!digits.is_empty(), "{error}");
    let scale = exponent
        .checked_add(unit)
        .and_then(|n| n.checked_sub(fraction.unwrap_or(0)))
        .context(error)?;
    let length = (digits.len() as i64).checked_add(scale).context(error)?;
    ensure!((1..=19).contains(&length), "{error}");
    let mut integer = digits.chars().take(length as usize).collect::<String>();
    while integer.len() < length as usize {
        integer.push('0');
    }
    let value = integer.parse::<u64>().context(error)?;
    ensure!(value > 0 && value <= i64::MAX as u64, "{error}");
    Ok(value)
}
fn mode<'a>(name: &'a str, m: &'a ArgMatches) -> &'a str {
    if name == "convert" {
        get(m, "mode").unwrap()
    } else {
        name
    }
}
fn check(name: &str, m: &ArgMatches) -> Result<()> {
    for (key, min, max) in [
        ("threads", 1, usize::MAX as u64),
        ("chunk-size", 1, usize::MAX as u64),
        ("batch-rows", 1, u32::MAX as u64),
        ("min-mapq", 0, 255),
        ("min-order", 1, usize::MAX as u64),
        ("max-order", 2, usize::MAX as u64),
        ("max-issues", 1, usize::MAX as u64),
        ("limit", 0, u64::MAX),
    ] {
        if let Some(v) = get(m, key) {
            let n = v
                .parse::<u64>()
                .with_context(|| format!("--{key} requires an integer"))?;
            ensure!((min..=max).contains(&n), "--{key} must be in {min}..={max}");
        }
    }
    if let Some(columns) = get(m, "columns") {
        let mut seen = std::collections::HashSet::new();
        ensure!(
            columns
                .split(',')
                .all(|c| !c.trim().is_empty() && seen.insert(c.trim())),
            "--columns requires distinct nonempty column names"
        );
    }
    for r in list(m, "region") {
        region(&r)?;
    }
    for r in list(m, "read-index") {
        r.parse::<u64>().context("--read-index requires UInt64")?;
    }
    if name == "query" {
        ensure!(
            !list(m, "region").is_empty(),
            "query requires at least one --region CHROM:START-END"
        );
    }
    if MODES.contains(&name) || name == "convert" {
        let mode = mode(name, m);
        if mode == "pairs2cool" {
            bin_size(get(m, "bin-size").context("pairs2cool requires --bin-size")?)?;
            ensure!(
                !on(m, "include-secondary")
                    && ["min-order", "max-order", "samtools", "pair-position"]
                        .iter()
                        .all(|k| get(m, k).is_none()),
                "pairs2cool does not accept alignment options"
            );
        } else {
            ensure!(
                get(m, "bin-size").is_none(),
                "--bin-size requires pairs2cool"
            );
            let minimum = if mode.ends_with("2pairs") { 2 } else { 1 };
            ensure!(
                num(m, "min-order", minimum) >= minimum,
                "--min-order must be at least {minimum}"
            );
            ensure!(
                num(m, "max-order", u64::MAX) > num(m, "min-order", minimum),
                "--max-order must exceed --min-order"
            );
            if mode == "concat2pairs" {
                ensure!(
                    !on(m, "include-secondary")
                        && ["contigsizes", "samtools", "tmpdir", "pair-position"]
                            .iter()
                            .all(|k| get(m, k).is_none()),
                    "alignment input options cannot be used with concat2pairs"
                );
            }
            ensure!(
                get(m, "contigsizes").is_none() || mode.starts_with("paf"),
                "--contigsizes requires PAF input"
            );
            ensure!(
                get(m, "samtools").is_none() || mode.starts_with("bam"),
                "--samtools requires BAM input"
            );
            ensure!(
                get(m, "pair-position").is_none() || ["bam2pairs", "paf2pairs"].contains(&mode),
                "--pair-position requires bam2pairs or paf2pairs"
            );
        }
    }
    Ok(())
}
fn json(text: &str) -> Result<Value> {
    Ok(Value::Object(Metadata::parse(text)?.fields))
}
fn print(value: &Value) -> Result<()> {
    writeln!(io::stdout().lock(), "{}", value.json())?;
    Ok(())
}
fn filter(m: &ArgMatches) -> Option<ConcatFilter> {
    get(m, "mode").map(|v| {
        if v == "complete-reads" {
            ConcatFilter::CompleteReads
        } else {
            ConcatFilter::MatchingAlignments
        }
    })
}
fn pairs(m: &ArgMatches) -> PairsMode {
    if get(m, "pairs-mode") == Some("both") {
        PairsMode::Both
    } else {
        PairsMode::Either
    }
}
fn run(name: &str, m: &ArgMatches) -> Result<i32> {
    let input = get(m, "input").unwrap();
    let output = get(m, "output").unwrap_or("-");
    let batch_rows = num(m, "batch-rows", 65536) as usize;
    let chunk_size = num(m, "chunk-size", 1_000_000) as usize;
    let threads = num(m, "threads", 1) as usize;
    let min_mapq = num(m, "min-mapq", 0) as u8;
    let mut status = 0;
    let report = match name {
        "inspect" => inspect(input)?.to_value(),
        "validate" => {
            let result = validate(
                input,
                if get(m, "level") == Some("full") {
                    ValidationLevel::Full
                } else {
                    ValidationLevel::Quick
                },
                num(m, "max-issues", 100) as usize,
            )?;
            status = match result.status {
                "valid" => 0,
                "invalid" => 1,
                _ => 3,
            };
            result.to_value()
        }
        "info" | "stats" => {
            let value = if name == "info" {
                presentation::info(input, on(m, "stats"))?
            } else {
                statistics::stats(input, min_mapq)?
            };
            if !on(m, "json") {
                crate::cli_display::table(name, &value)?;
                return Ok(0);
            }
            value
        }
        "head" | "view" | "export" | "query" => {
            let limit = if name == "view" && on(m, "all") {
                None
            } else {
                get(m, "limit")
                    .map(|v| v.parse::<u64>().unwrap())
                    .or(match name {
                        "head" => Some(10),
                        "view" => Some(100),
                        _ => None,
                    })
            };
            let options = presentation::ExportOptions {
                auto_index: name == "query" && !on(m, "no-build-index"),
                format: get(m, "format").unwrap_or("tsv").into(),
                columns: get(m, "columns")
                    .map(|s| s.split(',').map(|s| s.trim().to_owned()).collect()),
                limit,
                header: !on(m, "no-header"),
                threads,
                query: QueryOptions {
                    regions: list(m, "region")
                        .iter()
                        .map(|r| region(r))
                        .collect::<Result<_>>()?,
                    min_mapq,
                    index: match get(m, "index") {
                        Some("off") => IndexMode::Off,
                        Some("require") => IndexMode::Require,
                        _ => IndexMode::Auto,
                    },
                    pairs_mode: pairs(m),
                    read: ReadOptions {
                        batch_rows,
                        concat_filter: filter(m),
                        ..Default::default()
                    },
                },
            };
            let result = presentation::export(
                input,
                if output == "-" {
                    None
                } else {
                    Some(Path::new(output))
                },
                options,
            )?;
            if on(m, "show-stats") {
                writeln!(
                    io::stderr().lock(),
                    "{}",
                    result.object().unwrap()["query_stats"].json()
                )?;
            }
            if output == "-" {
                return Ok(0);
            }
            result
        }
        "merge" => merge(
            &list(m, "input"),
            output,
            MergeOptions {
                chunk_size,
                batch_rows,
                provenance: !on(m, "no-provenance"),
            },
        )?
        .to_value(),
        "subset" => subset(
            input,
            output,
            SubsetOptions {
                min_mapq: get(m, "min-mapq").map(|_| min_mapq),
                chroms: if list(m, "chrom").is_empty() {
                    None
                } else {
                    Some(list(m, "chrom"))
                },
                regions: if list(m, "region").is_empty() {
                    None
                } else {
                    Some(
                        list(m, "region")
                            .iter()
                            .map(|r| region(r))
                            .collect::<Result<_>>()?,
                    )
                },
                read_ids: if !list(m, "read-id").is_empty() {
                    Some(ReadIds::Pairs(list(m, "read-id")))
                } else if !list(m, "read-index").is_empty() {
                    Some(ReadIds::Concat(
                        list(m, "read-index")
                            .iter()
                            .map(|r| r.parse().unwrap())
                            .collect(),
                    ))
                } else {
                    None
                },
                pairs_mode: get(m, "pairs-mode").map(|_| pairs(m)),
                mode: filter(m),
                chunk_size,
                batch_rows,
                provenance: !on(m, "no-provenance"),
            },
        )?
        .to_value(),
        "index-build" | "index-status" | "index-rebuild" => {
            let qualities: &[IndexQuality] = match get(m, "quality") {
                Some("q0") => &[IndexQuality::Q0],
                Some("q1") => &[IndexQuality::Q1],
                _ => &[IndexQuality::Q0, IndexQuality::Q1],
            };
            let mut results = Vec::new();
            for &q in qualities {
                if name != "index-status" {
                    build_index_for_quality(input, q, name == "index-rebuild" || on(m, "rebuild"))?;
                }
                results.push(query::index_status(input, q)?);
            }
            Value::Object(
                [
                    ("path".into(), Value::from(input)),
                    ("indexes".into(), Value::List(results)),
                ]
                .into(),
            )
        }
        _ => {
            let mode = mode(name, m);
            if mode == "pairs2cool" {
                json(
                    &pairs2cool(
                        input,
                        output,
                        CoolOptions {
                            bin_size: bin_size(get(m, "bin-size").unwrap())?,
                            chunk_size,
                            batch_rows,
                            min_mapq,
                            threads,
                            contigsizes: path(m, "contigsizes"),
                            tmpdir: path(m, "tmpdir"),
                        },
                    )?
                    .to_json(),
                )?
            } else if mode == "concat2pairs" {
                json(
                    &convert(
                        input,
                        output,
                        mode,
                        ConvertOptions {
                            threads,
                            chunk_size,
                            batch_rows,
                            min_mapq,
                            min_order: num(m, "min-order", 2) as usize,
                            max_order: num(m, "max-order", usize::MAX as u64) as usize,
                        },
                    )?
                    .to_json(),
                )?
            } else {
                json(
                    &import_alignments(
                        input,
                        output,
                        mode,
                        ImportOptions {
                            threads,
                            chunk_size,
                            batch_rows,
                            min_mapq,
                            min_order: get(m, "min-order").map(|_| num(m, "min-order", 1) as usize),
                            max_order: get(m, "max-order")
                                .map(|_| num(m, "max-order", usize::MAX as u64) as usize),
                            contigsizes: path(m, "contigsizes"),
                            tmpdir: path(m, "tmpdir"),
                            include_secondary: on(m, "include-secondary"),
                            five_prime: get(m, "pair-position") == Some("five-prime"),
                        },
                    )?
                    .to_json(),
                )?
            }
        }
    };
    print(&report)?;
    Ok(status)
}
pub fn main() -> i32 {
    // Historical multi-character short aliases are normalized only as option
    // tokens; values and everything following -- retain their exact spelling.
    let mut args = Vec::new();
    let mut literal = false;
    let mut value = false;
    for arg in std::env::args_os() {
        let text = arg.to_str().unwrap_or("");
        if literal || value {
            value = false;
            args.push(arg);
            continue;
        }
        if text == "--" {
            literal = true;
        }
        let normalized = match text {
            "-help" | "-h" => "--help",
            "-bs" => "--bin-size",
            _ => text,
        };
        value = [
            "--output",
            "-o",
            "--mode",
            "--bin-size",
            "--binsize",
            "--min-mapq",
            "--min-order",
            "--max-order",
            "--threads",
            "-t",
            "--contigsizes",
            "--tmpdir",
            "--pair-position",
            "--samtools",
            "--region",
            "--columns",
            "--limit",
            "-n",
            "--index",
            "--quality",
            "--chrom",
            "--read-id",
            "--read-index",
            "--chunk-size",
            "--batch-rows",
            "--level",
            "--max-issues",
        ]
        .contains(&normalized);
        args.push(if normalized != text {
            normalized.into()
        } else {
            arg
        });
    }
    let matches = match app().try_get_matches_from(args) {
        Ok(m) => m,
        Err(e) => {
            let code = e.exit_code();
            let _ = e.print();
            return code;
        }
    };
    let Some((mut name, mut m)) = matches.subcommand() else {
        let _ = app().print_help();
        return 0;
    };
    if name == "index" {
        let Some((sub, child)) = m.subcommand() else {
            let mut root = app();
            let _ = root.find_subcommand_mut("index").unwrap().print_help();
            return 0;
        };
        name = match sub {
            "build" => "index-build",
            "status" => "index-status",
            _ => "index-rebuild",
        };
        m = child;
    }
    if let Err(e) = check(name, m) {
        eprintln!("Error: {e:#}\nTry 'pqsio {name} --help'.");
        return 2;
    }
    let enabled = if on(m, "progress") {
        Some(true)
    } else if on(m, "no-progress") {
        Some(false)
    } else {
        None
    };
    let progress = crate::cli_display::Progress::new(enabled, name);
    let result = run(name, m);
    let broken_pipe = result.as_ref().err().is_some_and(|e| {
        e.chain().any(|e| {
            e.downcast_ref::<io::Error>()
                .is_some_and(|e| e.kind() == io::ErrorKind::BrokenPipe)
        })
    });
    progress.finish(if broken_pipe {
        None
    } else {
        Some(result.is_ok())
    });
    match result {
        Ok(code) => code,
        Err(e)
            if e.chain().any(|e| {
                e.downcast_ref::<io::Error>()
                    .is_some_and(|e| e.kind() == io::ErrorKind::BrokenPipe)
            }) =>
        {
            0
        }
        Err(e) => {
            eprintln!("Error: {e:#}");
            if e.is::<pqsio::import::InvalidInput>() || e.is::<pqsio::cool::InvalidInput>() {
                2
            } else {
                1
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_binsizes() {
        for (s, n) in [
            ("10k", 10000),
            ("1.5m", 1500000),
            ("1e3", 1000),
            ("0.0019k", 1),
            ("9223372036854775807", i64::MAX as u64),
            ("9223372036854775.807k", i64::MAX as u64),
        ] {
            assert_eq!(bin_size(s).unwrap(), n);
        }
        for s in [
            "",
            "0",
            "-1m",
            "NaN",
            "inf",
            "10kb",
            "0.0001k",
            "9223372036854775808",
            "9223372036854775.808k",
            "1e1000000000g",
        ] {
            assert!(bin_size(s).is_err(), "{s}");
        }
    }
    #[test]
    fn regions() {
        assert_eq!(
            region("ref:chr-1:0-18446744073709551615").unwrap().end,
            u64::MAX
        );
        assert!(region("a:10-10").is_err());
    }
    #[test]
    fn clap_consistent() {
        app().debug_assert();
    }
}
