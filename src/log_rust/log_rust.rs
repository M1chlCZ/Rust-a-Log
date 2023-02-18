use std::fs::OpenOptions;
use std::io;
use std::io::Write;
use chrono::{DateTime, Local};

pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
    Trace,
}

impl Level {
    fn to_string(&self) -> &str {
        match self {
            Level::Trace => "[TRACE]",
            Level::Debug => "[DEBUG]",
            Level::Info => "[INFO]",
            Level::Warn => "[WARNING]",
            Level::Error => "[ERROR]",
        }
    }
}
#[derive(Debug)]
#[derive(PartialEq)]
pub struct Logger {
    path: String,
}


impl Logger {
    pub fn new(path: &str) -> Result<Self, io::Error> {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        Ok(Logger {
            path: path.to_string(),
        })
    }

    pub fn log_message(&self, message: &str, level: Level) -> Result<(),io::Error> {
        let now: DateTime<Local> = Local::now();
        let log_message = format!(
            "{} {} {}\n",
            now.format("%Y/%m/%d %H:%M:%S"),
            level.to_string(),
            message
        );

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;

        file.write_all(log_message.as_bytes())?;
        Ok(())
    }
}



// pub fn log(message: &str, level: Level) {
//     let now: DateTime<Local> = Local::now();
//     let log_message = format!(
//         "{} {} {}\n",
//         now.format("%Y/%m/%d %H:%M:%S"),
//         level.to_string(),
//         message
//     );
//
//     let mut file = OpenOptions::new()
//         .create(true)
//         .append(true)
//         .open("api.log")
//         .expect("Failed to open log file");
//
//     file.write_all(log_message.as_bytes())
//         .expect("Failed to write log message to file");
// }