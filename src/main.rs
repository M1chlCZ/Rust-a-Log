use colored::*;
use std::env;
use std::fs::File;
use std::io::{self, BufRead, BufReader, ErrorKind, Seek, SeekFrom};
use std::path::Path;
use std::thread::sleep;
use std::time::Duration;

fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().collect();

    if args.len() != 2 {
        println!("Usage: {} <log_file>", args[0]);
        Err(io::Error::new(ErrorKind::Other, "Invalid arguments"))?;
    }

    let path = Path::new(&args[1]).to_str().unwrap().to_string();
    let file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return Err(io::Error::new(io::ErrorKind::Other, "Err: 69 | No such file - wrong file path"))
    };
    dry_run(&file)?;
    loop_run(&file)?;
    Ok(())
}

fn dry_run(path: &File) -> io::Result<()> {
    let reader = BufReader::new(path);
    let lines: Vec<String> = reader.lines().map(|l| l.unwrap()).collect();
    let end: usize = lines.len();
    let start = if lines.len() > 10 {
        end - 10
    } else {
        end - lines.len()
    };

    for line in lines[start..end].iter() {
        let buffer = line.as_str();
        let log_level_start = buffer.find("[").unwrap();
        let log_level_end = buffer.find("]").unwrap();
        let log_level = &buffer[log_level_start..log_level_end + 1];
        let date = &buffer[0..log_level_start].white().hidden();
        let message = &buffer[log_level_end + 1..].white().bold();

        let colored_level = match log_level {
            "[ERROR]" => log_level.bright_red().bold(),
            "[WARN]" => log_level.yellow().bold(),
            "[INFO]" => log_level.green().bold(),
            _ => log_level.normal(),
        };

        let colored_line = format!("{}{}{}", date, colored_level, message);
        println!("{}", colored_line);
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
            let buffer = line.unwrap();
            let log_level_start = buffer.find("[").unwrap();
            let log_level_end = buffer.find("]").unwrap();
            let log_level = &buffer[log_level_start..log_level_end + 1];
            let first_part = &buffer[0..log_level_start];
            let message = &buffer[log_level_end + 1..];

            let colored_level = match log_level {
                "[ERROR]" => log_level.red().bold(),
                "[WARNING]" => log_level.yellow().bold(),
                "[INFO]" => log_level.green().bold(),
                _ => log_level.normal(),
            };

            let colored_line = format!("{}{}{}", first_part, colored_level, message);
            println!("{}", colored_line);
        }
    }
}
