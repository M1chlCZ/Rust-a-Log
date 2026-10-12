use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
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
// A bounded physical file line can expand sixfold through JSON escaping.
const MAX_RESPONSE: usize = MAX_FRAME * 6 + 1024;

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
        #[serde(default)]
        token: Option<String>,
    },
    Subscribe {
        version: u8,
        #[serde(default)]
        token: Option<String>,
    },
    Log {
        #[serde(flatten)]
        record: Record,
    },
    Dropped {
        count: u64,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
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

#[derive(Clone, Serialize, Deserialize)]
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

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum Response {
    Ready {
        version: u8,
    },
    Status {
        sources: VecDeque<Source>,
        evicted: u64,
        rejected: u64,
        adapter_dropped: u64,
        last_error: String,
    },
    Event {
        event: Event,
    },
}

pub fn token_from_env(name: &str) -> io::Result<Option<String>> {
    match std::env::var(name) {
        Ok(token)
            if !token.is_empty() && token.len() <= 256 && !token.chars().any(char::is_control) =>
        {
            Ok(Some(token))
        }
        Err(std::env::VarError::NotPresent) if name == "RUAL_TOKEN" => Ok(None),
        _ => Err(invalid(
            "token environment variable must contain 1 to 256 bytes without control characters",
        )),
    }
}

fn authenticated(expected: &Option<String>, supplied: &Option<String>) -> bool {
    let Some(expected) = expected else {
        return true;
    };
    let supplied = supplied.as_deref().unwrap_or("");
    let mut difference = expected.len() ^ supplied.len();
    for index in 0..256 {
        difference |= usize::from(
            expected.as_bytes().get(index).copied().unwrap_or(0)
                ^ supplied.as_bytes().get(index).copied().unwrap_or(0),
        );
    }
    difference == 0
}

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
        self.last_error = error.to_string().chars().take(512).collect();
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
    frame_limit(reader, pending, MAX_FRAME)
}

fn frame_limit(
    reader: &mut impl BufRead,
    pending: &mut Vec<u8>,
    limit: usize,
) -> io::Result<Option<Vec<u8>>> {
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
    if pending.len() + size > limit {
        return Err(invalid("frame exceeds size limit"));
    }
    pending.extend_from_slice(&buffer[..size]);
    reader.consume(size);
    if newline.is_some() {
        return Ok(Some(std::mem::take(pending)));
    }
    // Check cancellation and handshake deadlines between partial reads.
    Ok(None)
}

pub struct Server {
    pub address: SocketAddr,
    pub state: Shared,
    pub stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Server {
    pub fn bind(address: SocketAddr, token: Option<String>) -> io::Result<Self> {
        if !address.ip().is_loopback() && token.is_none() {
            return Err(invalid(
                "LAN receiver requires a token in RUAL_TOKEN or --token-env",
            ));
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
                        let (state, stop, active, token) = (
                            shared.clone(),
                            shutdown.clone(),
                            active.clone(),
                            token.clone(),
                        );
                        workers.push(thread::spawn(move || {
                            handle(stream, &state, &stop, &token);
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

fn handle(stream: TcpStream, state: &Shared, stop: &AtomicBool, token: &Option<String>) {
    let mut id = None;
    let result = (|| -> io::Result<()> {
        stream.set_read_timeout(Some(Duration::from_millis(100)))?;
        stream.set_write_timeout(Some(Duration::from_millis(200)))?;
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
                        token: supplied,
                    },
                ) => {
                    if !authenticated(token, &supplied) {
                        return Err(invalid("authentication failed"));
                    }
                    id = Some(state.lock().unwrap().register(source, language)?);
                }
                (
                    None,
                    Wire::Subscribe {
                        version: 1,
                        token: supplied,
                    },
                ) => {
                    if !authenticated(token, &supplied) {
                        return Err(invalid("authentication failed"));
                    }
                    return subscribe(reader.get_mut(), state, stop);
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

fn write_response(stream: &mut TcpStream, response: &Response) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(response)?;
    if bytes.len() + 1 > MAX_RESPONSE {
        return Err(invalid("response exceeds size limit"));
    }
    bytes.push(b'\n');
    let deadline = Instant::now() + Duration::from_millis(200);
    let mut pending = bytes.as_slice();
    while !pending.is_empty() {
        if Instant::now() > deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "viewer write timeout",
            ));
        }
        let written = stream.write(pending)?;
        if written == 0 {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "viewer closed"));
        }
        pending = &pending[written..];
    }
    Ok(())
}

fn subscribe(stream: &mut TcpStream, state: &Shared, stop: &AtomicBool) -> io::Result<()> {
    write_response(stream, &Response::Ready { version: 1 })?;
    let mut sequence = 0;
    while !stop.load(Ordering::Relaxed) {
        let (status, events) = {
            let state = state.lock().unwrap();
            (
                Response::Status {
                    sources: state.sources.clone(),
                    evicted: state.evicted,
                    rejected: state.rejected,
                    adapter_dropped: state.adapter_dropped,
                    last_error: state.last_error.clone(),
                },
                state
                    .events
                    .iter()
                    .filter(|event| event.sequence > sequence)
                    .cloned()
                    .collect::<Vec<_>>(),
            )
        };
        write_response(stream, &status)?;
        for event in events {
            if stop.load(Ordering::Relaxed) {
                return Ok(());
            }
            sequence = event.sequence;
            write_response(stream, &Response::Event { event })?;
        }
        thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

pub struct Remote {
    pub state: Shared,
    pub finished: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Remote {
    pub fn connect(address: &str, token: Option<String>) -> io::Result<Self> {
        let addresses: Vec<_> = address.to_socket_addrs()?.collect();
        if addresses.iter().any(|address| !address.ip().is_loopback()) && token.is_none() {
            return Err(invalid(
                "remote connection requires a token in RUAL_TOKEN or --token-env",
            ));
        }
        let mut stream = addresses
            .iter()
            .find_map(|address| TcpStream::connect_timeout(address, Duration::from_secs(2)).ok())
            .ok_or_else(|| io::Error::other("cannot connect to receiver"))?;
        stream.set_write_timeout(Some(Duration::from_millis(200)))?;
        stream.set_read_timeout(Some(Duration::from_millis(100)))?;
        writeln!(
            stream,
            "{}",
            serde_json::json!({"type":"subscribe", "version":1, "token":token})
        )?;
        let mut reader = BufReader::new(stream);
        let mut pending = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if Instant::now() > deadline {
                return Err(invalid("subscription timeout"));
            }
            match frame_limit(&mut reader, &mut pending, MAX_RESPONSE) {
                Ok(Some(bytes)) => {
                    if !matches!(
                        serde_json::from_slice::<Response>(&bytes),
                        Ok(Response::Ready { version: 1 })
                    ) {
                        return Err(invalid("invalid receiver response"));
                    }
                    break;
                }
                Ok(None) => continue,
                Err(_) => {
                    return Err(invalid(
                        "subscription refused; check the token and receiver version",
                    ));
                }
            }
        }
        let state = Shared::default();
        let shared = state.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let shutdown = stop.clone();
        let finished = Arc::new(AtomicBool::new(false));
        let done = finished.clone();
        let worker = thread::spawn(move || {
            let result = (|| -> io::Result<()> {
                while !shutdown.load(Ordering::Relaxed) {
                    let Some(bytes) = frame_limit(&mut reader, &mut pending, MAX_RESPONSE)? else {
                        continue;
                    };
                    let response: Response = serde_json::from_slice(&bytes)
                        .map_err(|_| invalid("invalid receiver response"))?;
                    let mut state = shared.lock().unwrap();
                    match response {
                        Response::Status {
                            sources,
                            evicted,
                            rejected,
                            adapter_dropped,
                            last_error,
                        } => {
                            if sources.len() > MAX_SOURCES || last_error.len() > MAX_FRAME {
                                return Err(invalid("receiver status exceeds limits"));
                            }
                            state.sources = sources;
                            state.evicted = state.evicted.max(evicted);
                            state.rejected = rejected;
                            state.adapter_dropped = adapter_dropped;
                            state.last_error = last_error;
                        }
                        Response::Event { mut event } => {
                            event.record.validate()?;
                            if event.source.len() > 128
                                || event.language.len() > 32
                                || event.sequence <= state.next_event
                            {
                                return Err(invalid("invalid receiver event"));
                            }
                            state.next_event = event.sequence;
                            event.bytes = serde_json::to_vec(&event.record)?.len();
                            state.bytes += event.bytes;
                            state.events.push_back(event);
                            while state.events.len() > MAX_RECORDS || state.bytes > MAX_BYTES {
                                state.bytes -= state.events.pop_front().unwrap().bytes;
                                state.evicted += 1;
                            }
                        }
                        Response::Ready { .. } => {
                            return Err(invalid("unexpected receiver greeting"));
                        }
                    }
                }
                Ok(())
            })();
            let mut state = shared.lock().unwrap();
            for source in &mut state.sources {
                source.connected = false;
            }
            if let Err(error) = result {
                state.last_error =
                    format!("receiver disconnected: {error}; run connect again to reconnect");
            }
            done.store(true, Ordering::Relaxed);
        });
        Ok(Self {
            state,
            finished,
            stop,
            worker: Some(worker),
        })
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
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
        let server = Server::bind("127.0.0.1:0".parse().unwrap(), None).unwrap();
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

    #[test]
    fn fragmented_registration_cannot_hold_shutdown_open() {
        let server = Server::bind("127.0.0.1:0".parse().unwrap(), None).unwrap();
        let mut stream = TcpStream::connect(server.address).unwrap();
        let worker = thread::spawn(move || {
            for _ in 0..200 {
                if stream.write_all(b" ").is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
        });
        thread::sleep(Duration::from_millis(100));
        let start = Instant::now();
        drop(server);
        assert!(start.elapsed() < Duration::from_secs(1));
        worker.join().unwrap();
    }
}
