use clap::{Arg, ArgAction, Command, value_parser};
use colored::Colorize;
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

mod tail;

const LEVELS: [(&str, char, &str); 6] = [
    ("errors", 'e', "ERROR"),
    ("warnings", 'w', "WARNING"),
    ("info", 'i', "INFO"),
    ("success", 's', "SUCCESS"),
    ("debug", 'd', "DEBUG"),
    ("trace", 't', "TRACE"),
];

fn cli() -> Command {
    let mut command = Command::new("rual")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Read recent log records quickly, filter them, and follow new writes")
        .arg(
            Arg::new("file")
                .required(true)
                .value_parser(value_parser!(PathBuf)),
        )
        .arg(
            Arg::new("count")
                .value_name("LINES")
                .value_parser(value_parser!(usize)),
        )
        .arg(
            Arg::new("lines")
                .short('n')
                .long("lines")
                .value_name("LINES")
                .value_parser(value_parser!(usize))
                .conflicts_with("count")
                .help("Number of matching records to show (default: 10; 0: new records only)"),
        )
        .arg(
            Arg::new("follow")
                .short('f')
                .long("follow")
                .value_parser(value_parser!(bool))
                .num_args(0..=1)
                .default_missing_value("true")
                .default_value("true")
                .help("Follow new records (default); --follow false exits after reading"),
        )
        .arg(
            Arg::new("once")
                .long("once")
                .action(ArgAction::SetTrue)
                .conflicts_with("follow")
                .help("Print recent records and exit"),
        )
        .arg(
            Arg::new("contains")
                .short('g')
                .long("contains")
                .value_name("TEXT")
                .help("Only show records containing this literal text"),
        )
        .arg(
            Arg::new("ignore-case")
                .long("ignore-case")
                .action(ArgAction::SetTrue)
                .requires("contains")
                .help("Ignore case when searching text"),
        )
        .arg(
            Arg::new("color")
                .long("color")
                .value_parser(["auto", "always", "never"])
                .default_value("auto")
                .help("Color output (auto respects NO_COLOR and pipes)"),
        );
    for (name, short, _) in LEVELS {
        command = command.arg(
            Arg::new(name)
                .long(name)
                .short(short)
                .value_parser(value_parser!(usize))
                .num_args(0..=1)
                .default_missing_value("10")
                .value_name("LINES")
                .help("Include this level; optionally set the legacy record count"),
        );
    }
    command
}

struct Filter {
    levels: Vec<&'static str>,
    text: Option<String>,
    ignore_case: bool,
}

impl Filter {
    fn matches(&self, line: &str) -> bool {
        (self.levels.is_empty()
            || level_span(line).is_some_and(|(_, _, level)| self.levels.contains(&level)))
            && self.text.as_ref().is_none_or(|text| {
                if self.ignore_case {
                    line.to_lowercase().contains(text)
                } else {
                    line.contains(text)
                }
            })
    }
}

// Find a recognized level even when the record starts with a bracketed timestamp.
fn level_span(line: &str) -> Option<(usize, usize, &'static str)> {
    for (start, _) in line.match_indices('[') {
        let rest = &line[start + 1..];
        // Bound the search so malformed records with many '[' stay linear.
        let Some(length) = rest
            .as_bytes()
            .iter()
            .take(8)
            .position(|&byte| byte == b']')
        else {
            continue;
        };
        let level = match &rest[..length] {
            "ERROR" => "ERROR",
            "WARN" | "WARNING" => "WARNING",
            "INFO" => "INFO",
            "SUCCESS" => "SUCCESS",
            "DEBUG" => "DEBUG",
            "TRACE" => "TRACE",
            _ => continue,
        };
        return Some((start, start + length + 2, level));
    }
    None
}

fn print_record(output: &mut impl Write, line: &str) -> io::Result<()> {
    let Some((start, end, level)) = level_span(line) else {
        return writeln!(output, "{line}");
    };
    let label = &line[start..end];
    let label = match level {
        "ERROR" => label.red().bold(),
        "WARNING" => label.yellow().bold(),
        "SUCCESS" => label.green().bold(),
        "DEBUG" => label.cyan().bold(),
        "TRACE" => label.magenta().bold(),
        _ => label.bright_white().bold(),
    };
    writeln!(output, "{}{}{}", &line[..start], label, &line[end..])
}

fn run() -> io::Result<()> {
    let args = cli().get_matches();
    match args.get_one::<String>("color").map(String::as_str) {
        Some("always") => colored::control::set_override(true),
        Some("never") => colored::control::set_override(false),
        _ => {}
    }
    let path = args.get_one::<PathBuf>("file").unwrap();
    let count = args
        .get_one::<usize>("lines")
        .or_else(|| args.get_one::<usize>("count"))
        .copied()
        .unwrap_or_else(|| {
            LEVELS
                .iter()
                .filter_map(|(name, _, _)| args.get_one::<usize>(name).copied())
                .max()
                .unwrap_or(10)
        });
    let ignore_case = args.get_flag("ignore-case");
    let filter = Filter {
        levels: LEVELS
            .iter()
            .filter(|(name, _, _)| args.contains_id(name))
            .map(|(_, _, level)| *level)
            .collect(),
        text: args.get_one::<String>("contains").map(|text| {
            if ignore_case {
                text.to_lowercase()
            } else {
                text.clone()
            }
        }),
        ignore_case,
    };
    let follow = !args.get_flag("once") && *args.get_one::<bool>("follow").unwrap();
    let mut file = File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a regular log file",
        ));
    }
    let end = file.metadata()?.len();
    let start = tail::find_start(&mut file, end, count, |line| filter.matches(line))?;
    file.seek(SeekFrom::Start(start))?;
    let mut output = io::BufWriter::new(io::stdout().lock());
    let mut emit = |line: &str| {
        if filter.matches(line) {
            print_record(&mut output, line)?;
        }
        Ok(())
    };
    let mut pending = Vec::new();
    // Limit the initial read to the snapshot used by the backwards scan.
    tail::read_records(
        &mut BufReader::new((&mut file).take(end - start)),
        &mut pending,
        &mut emit,
    )?;
    if !follow && !pending.is_empty() {
        emit(&String::from_utf8_lossy(&pending))?;
    }
    output.flush()?;
    if follow {
        tail::follow(path, file, pending, |line| {
            if filter.matches(line) {
                print_record(&mut output, line)?;
                output.flush()?;
            }
            Ok(())
        })?;
    }
    Ok(())
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("rual: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
