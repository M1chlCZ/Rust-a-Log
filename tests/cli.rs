use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

static NEXT_FILE: AtomicUsize = AtomicUsize::new(0);

struct LogFile(PathBuf);

impl LogFile {
    fn new(bytes: &[u8]) -> Self {
        let directory = std::env::temp_dir().join(format!(
            "rual-test-{}-{}",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("test.log");
        fs::write(&path, bytes).unwrap();
        Self(path)
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_rual"))
            .arg(&self.0)
            .args(args)
            .env("NO_COLOR", "1")
            .output()
            .unwrap()
    }

    fn append(&self, bytes: &[u8]) {
        fs::OpenOptions::new()
            .append(true)
            .open(&self.0)
            .unwrap()
            .write_all(bytes)
            .unwrap();
    }
}

struct Following {
    child: Child,
    lines: Receiver<String>,
}

impl Following {
    fn new(log: &LogFile) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_rual"))
            .arg(&log.0)
            .arg("-e")
            .env("NO_COLOR", "1")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let output = child.stdout.take().unwrap();
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                if sender.send(line.unwrap()).is_err() {
                    break;
                }
            }
        });
        Self { child, lines }
    }

    fn expect(&self, expected: &str) {
        assert_eq!(
            self.lines.recv_timeout(Duration::from_secs(5)).unwrap(),
            expected
        );
    }
}

impl Drop for Following {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for LogFile {
    fn drop(&mut self) {
        fs::remove_dir_all(self.0.parent().unwrap()).unwrap();
    }
}

fn stdout(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn legacy_filter_returns_exactly_the_requested_count() {
    let log = LogFile::new(b"[ERROR] first\n[ERROR] second\n[ERROR] third\n");
    assert_eq!(
        stdout(log.run(&["--errors", "2", "--follow", "false"])),
        "[ERROR] second\n[ERROR] third\n"
    );
}

#[test]
fn malformed_brackets_do_not_crash_the_viewer() {
    let log = LogFile::new(b"] before [INFO\n");
    assert_eq!(stdout(log.run(&["--follow", "false"])), "] before [INFO\n");
}

#[test]
fn tail_filters_text_and_validates_arguments() {
    let log = LogFile::new(b"[2026] [ERROR] First timeout\n[WARN] TIMEOUT\nINFO is just text\n[INFO] timeout\n[DEBUG] timeout\n");
    assert_eq!(
        stdout(log.run(&[
            "--once",
            "-e",
            "-w",
            "-n",
            "1",
            "-g",
            "timeout",
            "--ignore-case"
        ])),
        "[WARN] TIMEOUT\n"
    );
    assert_eq!(
        stdout(log.run(&["--once", "-i", "-n", "10"])),
        "[INFO] timeout\n"
    );
    assert_eq!(
        stdout(log.run(&["2", "--once"])),
        "[INFO] timeout\n[DEBUG] timeout\n"
    );
    assert_eq!(stdout(log.run(&["--once", "-n", "0"])), "");
    assert_eq!(stdout(log.run(&["--once", "-g", "absent"])), "");
    for args in [
        &["--once", "-n", "bad"][..],
        &["--once", "-n", "-1"],
        &["--ignore-case"],
    ] {
        assert!(!log.run(args).status.success());
    }
    assert!(!stdout(log.run(&["--once"])).contains('\x1b'));
    assert!(stdout(log.run(&["--once", "--color", "always"])).contains('\x1b'));
}

#[test]
fn empty_crlf_and_non_utf8_logs_are_readable() {
    for (bytes, expected) in [
        (&b""[..], ""),
        (&b"\n\n"[..], "\n\n"),
        (&b"one\r\ntwo\r\nlast"[..], "one\ntwo\nlast\n"),
        (&b"\xff\nfinal\r"[..], "\u{fffd}\nfinal\r\n"),
    ] {
        let log = LogFile::new(bytes);
        assert_eq!(stdout(log.run(&["--once"])), expected);
    }
}

#[test]
fn logger_appends_records_the_viewer_can_filter() {
    use rust_a_log::{Level, Logger};
    let log = LogFile::new(b"existing\n");
    let logger = Logger::new(log.0.to_str().unwrap()).unwrap();
    for (level, label) in [
        (Level::Info, "INFO"),
        (Level::Error, "ERROR"),
        (Level::Warn, "WARNING"),
        (Level::Debug, "DEBUG"),
        (Level::Trace, "TRACE"),
    ] {
        logger.log_message(label, level).unwrap();
    }
    let content = fs::read_to_string(&log.0).unwrap();
    assert!(content.starts_with("existing\n"));
    assert_eq!(content.lines().count(), 6);
    let lines = stdout(log.run(&["--once", "-e"]));
    assert_eq!(lines.lines().count(), 1);
    assert!(lines.ends_with(" [ERROR] ERROR\n"));
}

#[test]
fn follow_handles_partial_appends_and_truncation() {
    let log = LogFile::new(b"[ERROR] initial long record to make truncation observable\n");
    let following = Following::new(&log);
    following.expect("[ERROR] initial long record to make truncation observable");
    log.append(b"[INFO] ignored\n[ERROR] \xc5");
    assert!(
        following
            .lines
            .recv_timeout(Duration::from_millis(450))
            .is_err()
    );
    log.append(b"\xbe partial\n[ERROR] next\n");
    following.expect("[ERROR] ž partial");
    following.expect("[ERROR] next");
    fs::write(&log.0, b"[ERROR] reset\n").unwrap();
    following.expect("[ERROR] reset");
    log.append(b"[ERROR] unfinished");
    assert!(
        following
            .lines
            .recv_timeout(Duration::from_millis(450))
            .is_err()
    );
    fs::write(&log.0, b"").unwrap();
    std::thread::sleep(Duration::from_millis(450));
    log.append(b"[ERROR] after truncation\n");
    following.expect("[ERROR] after truncation");
}

#[cfg(unix)]
#[test]
fn follow_reopens_a_rotated_file_after_a_gap() {
    let log = LogFile::new(b"[ERROR] initial\n");
    let following = Following::new(&log);
    following.expect("[ERROR] initial");
    let rotated = log.0.with_extension("old");
    fs::rename(&log.0, &rotated).unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(&rotated)
        .unwrap()
        .write_all(b"[ERROR] drain old handle\n")
        .unwrap();
    following.expect("[ERROR] drain old handle");
    fs::write(
        &log.0,
        b"[ERROR] replacement longer than the original record\n[ERROR] second\n",
    )
    .unwrap();
    following.expect("[ERROR] replacement longer than the original record");
    following.expect("[ERROR] second");
    log.append(b"[ERROR] live\n");
    following.expect("[ERROR] live");
}

#[test]
fn a_closed_output_pipe_exits_without_a_panic() {
    let log = LogFile::new(&b"[INFO] record\n".repeat(100_000));
    let mut child = Command::new(env!("CARGO_BIN_EXE_rual"))
        .arg(&log.0)
        .args(["--once", "-n", "100000"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
}
