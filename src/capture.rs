use crate::live::{self, Event, Record, Shared};
use std::fs::File;
use std::io::{self, BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

pub fn cancellation() -> io::Result<Arc<AtomicBool>> {
    let interrupted = Arc::new(AtomicBool::new(false));
    let flag = interrupted.clone();
    ctrlc::set_handler(move || flag.store(true, Ordering::Relaxed)).map_err(io::Error::other)?;
    Ok(interrupted)
}

pub fn write_event(output: &mut impl Write, event: &Event, json: bool) -> io::Result<()> {
    if json {
        serde_json::to_writer(&mut *output, event)?;
        writeln!(output)?;
    } else {
        writeln!(
            output,
            "{} [{}] {}: {}",
            live::safe_text(&event.record.timestamp),
            event.record.level,
            live::safe_text(&event.source),
            live::safe_text(&event.record.message)
        )?;
    }
    Ok(())
}

pub fn print_live(
    state: &Shared,
    json: bool,
    mut finished: impl FnMut() -> bool,
) -> io::Result<()> {
    let mut output = io::BufWriter::new(io::stdout().lock());
    let (mut sequence, mut rejected, mut dropped) = (0, 0, 0);
    loop {
        let done = finished();
        let (events, errors, last_error, oldest, lost) = {
            let state = state.lock().unwrap();
            (
                state
                    .events
                    .iter()
                    .filter(|event| event.sequence > sequence)
                    .cloned()
                    .collect::<Vec<_>>(),
                state.rejected,
                state.last_error.clone(),
                state.events.front().map(|event| event.sequence),
                state.adapter_dropped,
            )
        };
        if let Some(oldest) = oldest
            && oldest > sequence + 1
        {
            eprintln!(
                "rual: {} records expired before output",
                oldest - sequence - 1
            );
        }
        if errors > rejected {
            eprintln!(
                "rual: rejected client ({errors} total): {}",
                live::safe_text(&last_error)
            );
            rejected = errors;
        }
        if lost > dropped {
            eprintln!("rual: adapters report {lost} discarded records");
            dropped = lost;
        }
        for event in events {
            write_event(&mut output, &event, json)?;
            sequence = event.sequence;
        }
        output.flush()?;
        if done {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(25));
    }
}

pub fn stdin(filter: &crate::Filter) -> io::Result<()> {
    let mut output = io::BufWriter::new(io::stdout().lock());
    stream_lines(&mut io::stdin().lock(), |line| {
        let line = live::safe_text(line);
        if filter.matches(&line) {
            crate::print_record(&mut output, &line)?;
            output.flush()?;
        }
        Ok(())
    })
}

fn stream_lines(
    reader: &mut impl BufRead,
    mut emit: impl FnMut(&str) -> io::Result<()>,
) -> io::Result<()> {
    let mut pending = Vec::new();
    loop {
        match live::frame(reader, &mut pending) {
            Ok(Some(bytes)) => {
                emit(String::from_utf8_lossy(&bytes).trim_end_matches(['\r', '\n']))?
            }
            Ok(None) => continue,
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                if !pending.is_empty() {
                    emit(&String::from_utf8_lossy(&pending))?;
                }
                return Ok(());
            }
            Err(error) => return Err(error),
        }
    }
}

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub fn run_command(args: &clap::ArgMatches) -> io::Result<u8> {
    if args.get_flag("tui")
        && (!io::IsTerminal::is_terminal(&io::stdin())
            || !io::IsTerminal::is_terminal(&io::stdout()))
    {
        return Err(io::Error::other("TUI requires an interactive terminal"));
    }
    let interrupted = cancellation()?;
    let mut command = args.get_many::<std::ffi::OsString>("command").unwrap();
    let mut child = ChildGuard(
        Command::new(command.next().unwrap())
            .args(command)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?,
    );
    let state = Shared::default();
    let id = state.lock().unwrap().register(
        args.get_one::<String>("name").unwrap().clone(),
        "process".into(),
    )?;
    let failed = Arc::new(AtomicBool::new(false));
    let workers: Vec<_> = [
        Box::new(child.0.stdout.take().unwrap()) as Box<dyn io::Read + Send>,
        Box::new(child.0.stderr.take().unwrap()) as Box<dyn io::Read + Send>,
    ]
    .into_iter()
    .enumerate()
    .map(|(index, reader)| {
        let state = state.clone();
        let failed = failed.clone();
        thread::spawn(move || {
            let result = stream_lines(&mut BufReader::new(reader), |line| {
                let mut record = Record::from_line(line);
                record.fields["stream"] = if index == 0 { "stdout" } else { "stderr" }.into();
                state.lock().unwrap().push(id, record);
                Ok(())
            });
            if result.is_err() {
                failed.store(true, Ordering::Relaxed);
            }
            result
        })
    })
    .collect();
    if args.get_flag("tui") {
        crate::tui::run(&state, "process capture")?;
        let _ = child.0.kill();
        state.lock().unwrap().disconnect(id);
        return Ok(0);
    } else {
        print_live(&state, args.get_flag("json"), || {
            interrupted.load(Ordering::Relaxed)
                || failed.load(Ordering::Relaxed)
                || workers.iter().all(|worker| worker.is_finished())
        })?;
    }
    if interrupted.load(Ordering::Relaxed) {
        return Ok(130);
    }
    if failed.load(Ordering::Relaxed) {
        return Err(io::Error::other(
            "process output exceeded the 64 KiB record limit or failed to read",
        ));
    }
    for worker in workers {
        worker
            .join()
            .map_err(|_| io::Error::other("capture worker failed"))??;
    }
    state.lock().unwrap().disconnect(id);
    let status = loop {
        if interrupted.load(Ordering::Relaxed) {
            return Ok(130);
        }
        if let Some(status) = child.0.try_wait()? {
            break status;
        }
        thread::sleep(Duration::from_millis(25));
    };
    Ok(status
        .code()
        .and_then(|code| u8::try_from(code).ok())
        .unwrap_or(1))
}

pub fn follow_file(path: &Path, state: Shared, stop: Arc<AtomicBool>) -> io::Result<()> {
    let mut file = File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("expected a regular file"));
    }
    let end = file.metadata()?.len();
    let start = crate::tail::find_plain_start(&mut file, end, 100)?;
    file.seek(SeekFrom::Start(start))?;
    let path = path.to_path_buf();
    let id = state
        .lock()
        .unwrap()
        .register(path.to_string_lossy().into(), "file".into())?;
    thread::spawn(move || {
        let result = crate::tail::follow_until(
            &path,
            file,
            Vec::new(),
            |line| {
                state.lock().unwrap().push(id, Record::from_line(line));
                Ok(())
            },
            || !stop.load(Ordering::Relaxed),
            live::MAX_FRAME,
        );
        let mut state = state.lock().unwrap();
        state.disconnect(id);
        if let Err(error) = result {
            state.last_error = format!("file {path:?}: {error}");
            state.rejected += 1;
        }
    });
    Ok(())
}
