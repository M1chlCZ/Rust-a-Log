use clap::{Arg, Command, value_parser};
use colored::*;
use std::{env, usize};
use std::fs::File;
use std::io::{self, BufRead, BufReader, ErrorKind, Seek, SeekFrom};
use std::path::Path;
use std::thread::sleep;
use std::time::Duration;

use crate::utils::file_utils;
mod utils;

fn main() -> io::Result<()> {
    let mut num_lines: usize = 10;
    let args: Vec<String> = env::args().collect();
    let has_flag = args.iter().any(|arg| arg.starts_with("-") || arg.starts_with("--"));
    if has_flag {
        let matches = Command::new("Rust-a-Log")
            .arg(Arg::new("errors")
                .long("errors").short('e')
                .value_name("INT")
                .value_parser(value_parser!(i32))
                .num_args(0..=1)
                .default_missing_value("0")
                .help("Only show errors"))
            .arg(Arg::new("info")
                .long("info").short('i')
                .value_name("INT")
                .value_parser(value_parser!(i32))
                .num_args(0..=1)
                .default_missing_value("0")
                .help("Only show info"))
            .arg(Arg::new("follow")
                .long("follow").short('f')
                .value_name("BOOL")
                .value_parser(value_parser!(bool))
                .num_args(0..=1)
                .default_missing_value("true")
                .help("Follow the log file"))
            .arg(Arg::new("file").index(1).required(true).value_name("STRING").help("File to read"))
            .get_matches();

        let mut filter: String = String::new();
        let num_err_match = match matches.get_one::<i32>("errors") {
            Some(n) => {
                filter = "ERROR".to_string();
                if n > &0 { n } else { &10 }
            }
            None => &0,
        };
        let num_err = usize::try_from(*num_err_match).unwrap_or_default();
        let num_info_match = match matches.get_one::<i32>("info") {
            Some(n) => {
                filter = "INFO".to_string();
                if n > &0 { n } else { &10 }
            }
            None => &0,
        };
        let follow = match matches.get_one::<bool>("follow") {
            Some(b) => b,
            None => &false,
        };
        let num_info = usize::try_from(*num_info_match).unwrap_or_default();
        let path = Path::new(matches.get_one::<String>("file").unwrap()).to_str().unwrap_or_default();
        let file = file_utils::open_file(path)?;
        let u: usize = if filter.eq("ERROR") {
            num_err
        } else {
            num_info
        };
        dry_run_filter(&file, u, filter.clone())?;
        if follow == &true {
            loop_run_filter(&file, filter)?;
        }

    } else {
        if args.len() < 2 {
            println!("Usage: {} <log_file>", args[0]);
            //empty error return
            Err(io::Error::new(
                ErrorKind::NotFound,
                "Err: 01 | No file specified",
            ))?;
        }

        let path = Path::new(&args[1]).to_str().unwrap();
        let file = file_utils::open_file(path)?;

        if args.len() == 3 {
            num_lines = match args.get(2) {
                Some(arg) => arg.parse::<usize>().unwrap_or(10),
                None => 10,
            };
        }
        dry_run(&file, num_lines)?;
        loop_run(&file)?;
    }
    Ok(())
}

fn dry_run(path: &File, lines_sub: usize) -> io::Result<()> {
    let reader = BufReader::new(path);
    let lines: Vec<String> = reader.lines().map(|l| l.unwrap()).collect();
    let end: usize = lines.len();
    let start = if lines.len() > lines_sub {
        end - lines_sub
    } else {
        end - lines.len()
    };

    for line in lines[start..end].iter() {
        let str = line_parse(&line.to_string());
        println!("{}", str);
    }
    Ok(())
}

fn dry_run_filter(path: &File, lines_sub: usize, filter: String) -> io::Result<()> {
    let reader = BufReader::new(path);
    let lines: Vec<String> = reader.lines().map(|l| l.unwrap()).collect();
    let mut lines_filtered: Vec<String>  = Vec::new();
    let mut end: usize = lines.len();
    let amount_of_lines = if lines_sub == 0 { 10 } else { lines_sub };
    let mut lines_parsed = 0;
    while lines_parsed <= amount_of_lines {
        if end == 0 { break;}
        let line = match lines.get(end - 1) {
            Some(l) => l,
            None => break,
        };
        if line.contains(&filter) {
            end -= 1;
            lines_parsed += 1;
            lines_filtered.push(line.clone());
        }else{
            end -= 1;
        }
    }
    lines_filtered.reverse();
    for line in lines_filtered.iter() {
        let str = line_parse(&line.to_string());
        println!("{}", str);
    }
    Ok(())
}

fn loop_run(file: &File) -> io::Result<()> {
    let mut reader = BufReader::new(file);
    loop {
        reader.seek(SeekFrom::End(0)).unwrap();
        let original_file_size = reader.seek(SeekFrom::Current(0)).unwrap();

        sleep(Duration::from_secs(1));

        let mut reader = BufReader::new(file);

        let current_file_size = reader.seek(SeekFrom::End(0)).unwrap();
        if current_file_size == original_file_size {
            continue;
        }

        reader.seek(SeekFrom::Start(original_file_size)).unwrap();

        for line in reader.lines() {
            let str = line_parse(&line.unwrap());
            println!("{}", str);
        }
    }
}

fn loop_run_filter(file: &File, filter: String) -> io::Result<()> {
    let mut reader = BufReader::new(file);
    loop {
        reader.seek(SeekFrom::End(0)).unwrap();
        let original_file_size = reader.seek(SeekFrom::Current(0)).unwrap();

        sleep(Duration::from_secs(1));

        let mut reader = BufReader::new(file);

        let current_file_size = reader.seek(SeekFrom::End(0)).unwrap();
        if current_file_size == original_file_size {
            continue;
        }

        reader.seek(SeekFrom::Start(original_file_size)).unwrap();

        for line in reader.lines() {
            let str = line_parse(&line.unwrap());
            if !str.contains(&filter) {
                continue;
            }
            println!("{}", str);
        }
    }
}

fn line_parse(line: &String) -> String {
    if line.is_empty() {
        return "".white().to_string();
    }
    let log_level_start = match line.find("[") {
        Some(pos) => pos,
        None => {
            return line.white().to_string();
        }
    };

    let log_level_end = match line.find("]") {
        Some(pos) => pos,
        None => {
            return line.white().to_string();
        }
    };
    let log_level = &line[log_level_start..log_level_end + 1];
    let date = &line[0..log_level_start].white();
    let message = &line[log_level_end + 1..].white().bold();

    let colored_level = match log_level {
        "[ERROR]" => log_level.red().bold(),
        "[WARNING]" => log_level.yellow().bold(),
        "[INFO]" => log_level.bright_white().bold(),
        "[SUCCESS]" => log_level.green().bold(),
        _ => log_level.normal(),
    };

    format!("{}{}{}", date, colored_level, message)
}
