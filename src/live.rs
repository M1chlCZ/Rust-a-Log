use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub const MAX_FRAME: usize = 64 * 1024;
pub const MAX_RECORDS: usize = 2_000;
pub const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_CLIENTS: usize = 32;
const MAX_SOURCES: usize = 128;
pub const DEFAULT_BIND: &str = "127.0.0.1:7331";

fn timestamp() -> String {
    chrono::Utc::now().to_rfc3339()
}
fn info() -> String {
    "INFO".into()
}
fn fields() -> Value {
    Value::Object(Default::default())
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Record {
    #[serde(default = "timestamp")]
    pub timestamp: String,
    #[serde(default = "info")]
    pub level: String,
    pub message: String,
    #[serde(default = "fields")]
    pub fields: Value,
}

impl Record {
    fn validate(&mut self) -> io::Result<()> {
        self.level = match self.level.to_ascii_uppercase().as_str() {
            "WARN" | "WARNING" => "WARNING",
            "FATAL" | "CRITICAL" | "ERROR" => "ERROR",
            "DEBUG" => "DEBUG",
            "INFO" => "INFO",
            "SUCCESS" => "SUCCESS",
            "TRACE" => "TRACE",
            _ => return Err(invalid("unknown log level")),
        }
        .into();
        if self.timestamp.len() > 128 || !self.fields.is_object() {
            return Err(invalid("timestamp is too long or fields is not an object"));
        }
        Ok(())
    }

    pub fn from_line(line: &str) -> Self {
        if let Ok(mut record) = serde_json::from_str::<Self>(line)
            && record.validate().is_ok()
        {
            return record;
        }
        let level = crate::level_span(line)
            .map(|(_, _, level)| level)
            .unwrap_or("INFO");
        Self {
            timestamp: timestamp(),
            level: level.into(),
            message: line.into(),
            fields: fields(),
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum Wire {
    Register {
        version: u8,
        source: String,
        language: String,
    },
    Log {
        #[serde(flatten)]
        record: Record,
    },
    Dropped {
        count: u64,
    },
}

#[derive(Clone, Debug, Serialize)]
pub struct Event {
    pub sequence: u64,
    pub session: u64,
    pub source: String,
    pub language: String,
    #[serde(flatten)]
    pub record: Record,
    #[serde(skip)]
    bytes: usize,
}

#[derive(Clone)]
pub struct Source {
    pub id: u64,
    pub name: String,
    pub language: String,
    pub connected: bool,
    pub records: u64,
    pub dropped: u64,
}

#[derive(Default)]
pub struct State {
    pub sources: VecDeque<Source>,
    pub events: VecDeque<Event>,
    pub evicted: u64,
    pub rejected: u64,
    pub adapter_dropped: u64,
    pub last_error: String,
    pub bytes: usize,
    next_source: u64,
    next_event: u64,
}

pub type Shared = Arc<Mutex<State>>;

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

impl State {
    pub fn register(&mut self, name: String, language: String) -> io::Result<u64> {
        if name.trim().is_empty()
            || name.len() > 128
            || language.is_empty()
            || language.len() > 32
            || name.chars().any(char::is_control)
            || language.chars().any(char::is_control)
        {
            return Err(invalid("source name or language is empty or too long"));
        }
        if self.sources.len() == MAX_SOURCES {
            let index = self
                .sources
                .iter()
                .position(|source| !source.connected)
                .ok_or_else(|| invalid("source limit reached"))?;
            self.sources.remove(index);
        }
        self.next_source += 1;
        self.sources.push_back(Source {
            id: self.next_source,
            name,
            language,
            connected: true,
            records: 0,
            dropped: 0,
        });
        Ok(self.next_source)
    }

    pub fn push(&mut self, id: u64, record: Record) {
        let Some(source) = self.sources.iter_mut().find(|source| source.id == id) else {
            return;
        };
        source.records += 1;
        self.next_event += 1;
        let bytes = serde_json::to_vec(&record)
            .map(|bytes| bytes.len())
            .unwrap_or(MAX_FRAME);
        self.events.push_back(Event {
            sequence: self.next_event,
            session: id,
            source: source.name.clone(),
            language: source.language.clone(),
            record,
            bytes,
        });
        self.bytes += bytes;
        while self.events.len() > MAX_RECORDS || self.bytes > MAX_BYTES {
            self.bytes -= self.events.pop_front().unwrap().bytes;
            self.evicted += 1;
        }
    }

    pub fn disconnect(&mut self, id: u64) {
        if let Some(source) = self.sources.iter_mut().find(|source| source.id == id) {
            source.connected = false;
        }
    }

    fn reject(&mut self, error: impl ToString) {
        self.rejected += 1;
        self.last_error = error.to_string();
    }
}

pub fn safe_text(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() && c != '\n' && c != '\t' {
                '�'
            } else {
                c
            }
        })
        .collect()
}

/// Limit framing before decoding JSON. Timeouts preserve fragmented records.
pub fn frame(reader: &mut impl BufRead, pending: &mut Vec<u8>) -> io::Result<Option<Vec<u8>>> {
    loop {
        let buffer = match reader.fill_buf() {
            Ok(buffer) => buffer,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        if buffer.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "stream closed",
            ));
        }
        let newline = buffer.iter().position(|&byte| byte == b'\n');
        let size = newline.map_or(buffer.len(), |index| index + 1);
        if pending.len() + size > MAX_FRAME {
            return Err(invalid("record exceeds 64 KiB"));
        }
        pending.extend_from_slice(&buffer[..size]);
        reader.consume(size);
        if newline.is_some() {
            return Ok(Some(std::mem::take(pending)));
        }
    }
}

pub struct Server {
    pub address: SocketAddr,
    pub state: Shared,
    pub stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Server {
    pub fn bind(address: SocketAddr) -> io::Result<Self> {
        if !address.ip().is_loopback() {
            return Err(invalid("receiver must bind to a loopback address"));
        }
        let listener = TcpListener::bind(address)?;
        let address = listener.local_addr()?;
        listener.set_nonblocking(true)?;
        let state = Shared::default();
        let stop = Arc::new(AtomicBool::new(false));
        let shared = state.clone();
        let shutdown = stop.clone();
        let worker = thread::spawn(move || {
            let active = Arc::new(AtomicUsize::new(0));
            let mut workers = Vec::new();
            while !shutdown.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        if active.load(Ordering::Relaxed) >= MAX_CLIENTS {
                            shared.lock().unwrap().reject("connection limit reached");
                            continue;
                        }
                        active.fetch_add(1, Ordering::Relaxed);
                        let (state, stop, active) =
                            (shared.clone(), shutdown.clone(), active.clone());
                        workers.push(thread::spawn(move || {
                            handle(stream, &state, &stop);
                            active.fetch_sub(1, Ordering::Relaxed);
                        }));
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(20))
                    }
                    Err(error) => {
                        shared.lock().unwrap().reject(error);
                        break;
                    }
                }
                let mut index = 0;
                while index < workers.len() {
                    if workers[index].is_finished() {
                        let _ = workers.swap_remove(index).join();
                    } else {
                        index += 1;
                    }
                }
            }
            for worker in workers {
                let _ = worker.join();
            }
        });
        Ok(Self {
            address,
            state,
            stop,
            worker: Some(worker),
        })
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn handle(stream: TcpStream, state: &Shared, stop: &AtomicBool) {
    let mut id = None;
    let result = (|| -> io::Result<()> {
        stream.set_read_timeout(Some(Duration::from_millis(100)))?;
        let mut reader = BufReader::new(stream);
        let mut pending = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !stop.load(Ordering::Relaxed) {
            if id.is_none() && Instant::now() > deadline {
                return Err(invalid("registration timeout"));
            }
            let Some(bytes) = frame(&mut reader, &mut pending)? else {
                continue;
            };
            let wire: Wire =
                serde_json::from_slice(&bytes).map_err(|error| invalid(&error.to_string()))?;
            match (id, wire) {
                (
                    None,
                    Wire::Register {
                        version: 1,
                        source,
                        language,
                    },
                ) => {
                    id = Some(state.lock().unwrap().register(source, language)?);
                }
                (Some(id), Wire::Log { mut record }) => {
                    record.validate()?;
                    state.lock().unwrap().push(id, record);
                }
                (Some(id), Wire::Dropped { count }) => {
                    let mut state = state.lock().unwrap();
                    state.adapter_dropped = state.adapter_dropped.saturating_add(count);
                    if let Some(source) = state.sources.iter_mut().find(|source| source.id == id) {
                        source.dropped = source.dropped.saturating_add(count);
                    }
                }
                _ => {
                    return Err(invalid(
                        "version 1 registration must come first and occur once",
                    ));
                }
            }
        }
        Ok(())
    })();
    let mut state = state.lock().unwrap();
    if let Some(id) = id {
        state.disconnect(id);
    }
    if let Err(error) = result
        && error.kind() != io::ErrorKind::UnexpectedEof
    {
        state.reject(error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    #[test]
    fn store_limits_records_bytes_sources_and_preserves_session_identity() {
        let mut state = State::default();
        let first = state.register("api".into(), "go".into()).unwrap();
        for _ in 0..MAX_RECORDS + 1 {
            state.push(first, Record::from_line("hello"));
        }
        assert_eq!(state.events.len(), MAX_RECORDS);
        assert_eq!(state.evicted, 1);
        for _ in 0..200 {
            state.push(first, Record::from_line(&"x".repeat(MAX_FRAME - 512)));
        }
        assert!(state.bytes <= MAX_BYTES);
        state.disconnect(first);
        for _ in 0..150 {
            let id = state.register("api".into(), "go".into()).unwrap();
            state.disconnect(id);
        }
        assert_eq!(state.sources.len(), MAX_SOURCES);
        assert!(state.sources.back().unwrap().id > first);
    }

    #[test]
    fn frame_limits_and_terminal_controls_are_safe() {
        let mut pending = Vec::new();
        assert!(frame(&mut Cursor::new(vec![b'x'; MAX_FRAME + 1]), &mut pending).is_err());
        assert_eq!(safe_text("\x1b[2Jhello\r\nworld"), "�[2Jhello�\nworld");
    }

    #[test]
    fn disconnect_and_shutdown_release_idle_clients() {
        let server = Server::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let mut stream = TcpStream::connect(server.address).unwrap();
        stream
            .write_all(
                b"{\"type\":\"register\",\"version\":1,\"source\":\"api\",\"language\":\"go\"}\n",
            )
            .unwrap();
        let until = Instant::now() + Duration::from_secs(3);
        while server.state.lock().unwrap().sources.is_empty() {
            assert!(Instant::now() < until);
            thread::sleep(Duration::from_millis(10));
        }
        let state = server.state.clone();
        drop(stream);
        while state.lock().unwrap().sources[0].connected {
            assert!(Instant::now() < until);
            thread::sleep(Duration::from_millis(10));
        }
        let _idle = TcpStream::connect(server.address).unwrap();
        let start = Instant::now();
        drop(server);
        assert!(start.elapsed() < Duration::from_secs(1));
    }
}
