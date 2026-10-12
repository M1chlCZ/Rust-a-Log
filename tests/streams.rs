use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

struct Listener {
    child: Child,
    lines: Receiver<String>,
    errors: Receiver<String>,
    address: String,
}

fn lines(reader: impl std::io::Read + Send + 'static) -> Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(reader).lines() {
            if sender.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    receiver
}

impl Listener {
    fn new() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_rual"))
            .args(["listen", "--bind", "127.0.0.1:0", "--json"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let output = lines(child.stdout.take().unwrap());
        let errors = lines(child.stderr.take().unwrap());
        let mut listener = Self {
            child,
            lines: output,
            errors,
            address: String::new(),
        };
        let ready = listener
            .errors
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        listener.address = ready
            .strip_prefix("rual listening on ")
            .expect(&ready)
            .to_string();
        listener
    }

    fn connect(&self, source: &str) -> TcpStream {
        let mut stream = TcpStream::connect(&self.address).unwrap();
        writeln!(
            stream,
            "{{\"type\":\"register\",\"version\":1,\"source\":\"{source}\",\"language\":\"test\"}}"
        )
        .unwrap();
        stream
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn stdin_is_a_stream_without_a_seek_or_a_final_newline() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rual"))
        .args(["-", "-e", "--once"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"[INFO] skip\n[ERROR] final")
        .unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(result.status.success());
    assert_eq!(result.stdout, b"[ERROR] final\n");
}

#[test]
fn receiver_handles_fragmented_frames_and_separate_sessions() {
    let listener = Listener::new();
    let mut first = listener.connect("worker");
    first
        .write_all(b"{\"type\":\"log\",\"level\":\"WARN\",\"message\":\"one\\n")
        .unwrap();
    first
        .write_all(b"two\",\"fields\":{\"request\":42}}\n")
        .unwrap();
    let record = listener.lines.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(record.contains("\"source\":\"worker\"") && record.contains("\"level\":\"WARNING\""));
    assert!(record.contains("one\\ntwo") && record.contains("\"request\":42"));
    let mut second = listener.connect("other");
    second
        .write_all(b"{\"type\":\"log\",\"level\":\"ERROR\",\"message\":\"second\"}\n")
        .unwrap();
    assert!(
        listener
            .lines
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .contains("\"source\":\"other\"")
    );
}

#[test]
fn malformed_clients_do_not_stop_other_sources() {
    let listener = Listener::new();
    let mut invalid = listener.connect("bad");
    invalid.write_all(b"invalid json\n").unwrap();
    assert!(
        listener
            .errors
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .contains("rejected")
    );
    let mut oversized = listener.connect("big");
    let _ = oversized.write_all(&vec![b'x'; 65_537]);
    assert!(
        listener
            .errors
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .contains("rejected")
    );
    let mut valid = listener.connect("good");
    valid
        .write_all(b"{\"type\":\"log\",\"message\":\"still alive\"}\n")
        .unwrap();
    assert!(
        listener
            .lines
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .contains("still alive")
    );
}

#[test]
fn listener_rejects_network_wide_bind_addresses() {
    let result = Command::new(env!("CARGO_BIN_EXE_rual"))
        .args(["listen", "--bind", "0.0.0.0:0"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("loopback"));
}

#[test]
fn subprocess_fixture() {
    if std::env::var_os("RUAL_TEST_CHILD").is_some() {
        println!("[INFO] stdout");
        eprintln!("[ERROR] stderr");
        std::process::exit(7);
    }
}

#[test]
fn process_capture_preserves_both_streams_and_the_child_exit_code() {
    let result = Command::new(env!("CARGO_BIN_EXE_rual"))
        .args(["run", "--json", "--"])
        .arg(std::env::current_exe().unwrap())
        .args(["--exact", "subprocess_fixture", "--nocapture"])
        .env("RUAL_TEST_CHILD", "1")
        .output()
        .unwrap();
    assert_eq!(
        result.status.code(),
        Some(7),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let output = String::from_utf8(result.stdout).unwrap();
    assert!(
        output.contains("[INFO] stdout") && output.contains("[ERROR] stderr"),
        "{output}"
    );
}

#[cfg(unix)]
#[test]
fn cancelling_plain_capture_stops_a_child_that_ignores_sigint() {
    for close_outputs in [false, true] {
        check_cancellation(close_outputs);
    }
}

#[cfg(unix)]
fn check_cancellation(close_outputs: bool) {
    let script = if close_outputs {
        "trap '' INT; printf '%s\\n' $$; exec 1>&- 2>&-; exec sleep 30"
    } else {
        "trap '' INT; printf '%s\\n' $$; exec sleep 30"
    };
    let mut child = Command::new(env!("CARGO_BIN_EXE_rual"))
        .args(["run", "--json", "--", "sh", "-c", script])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let output = lines(child.stdout.take().unwrap());
    let mut guard = Listener {
        child,
        lines: output,
        errors: mpsc::channel().1,
        address: String::new(),
    };
    let record: serde_json::Value =
        serde_json::from_str(&guard.lines.recv_timeout(Duration::from_secs(5)).unwrap()).unwrap();
    let pid = record["message"].as_str().unwrap();
    if close_outputs {
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        Command::new("kill")
            .args(["-INT", &guard.child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = guard.child.try_wait().unwrap() {
            break status;
        }
        assert!(std::time::Instant::now() < deadline, "capture did not stop");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.code(), Some(130));
    assert!(
        !Command::new("kill")
            .args(["-0", pid])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success(),
        "direct child survived"
    );
}
