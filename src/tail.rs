use std::borrow::Cow;
use std::fs::{self, File, Metadata};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::thread;
use std::time::Duration;

const BLOCK_SIZE: usize = 64 * 1024;

fn text(bytes: &[u8]) -> Cow<'_, str> {
    let bytes = bytes
        .strip_suffix(b"\n")
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
        .unwrap_or(bytes);
    String::from_utf8_lossy(bytes)
}

/// Scan backwards until enough matching records are found, keeping one block
/// and one record in memory. The caller supplies a fixed EOF for a growing file.
pub fn find_start(
    reader: &mut (impl Read + Seek),
    end: u64,
    count: usize,
    matches: impl Fn(&str) -> bool,
) -> io::Result<u64> {
    if count == 0 {
        return Ok(end);
    }
    let mut position = end;
    let mut block = [0; BLOCK_SIZE];
    let mut reversed = Vec::new();
    let mut found = 0;
    while position > 0 {
        let size = position.min(BLOCK_SIZE as u64) as usize;
        position -= size as u64;
        reader.seek(SeekFrom::Start(position))?;
        reader.read_exact(&mut block[..size])?;
        for (index, &byte) in block[..size].iter().enumerate().rev() {
            let offset = position + index as u64;
            if byte == b'\n' && offset + 1 != end {
                reversed.reverse();
                if matches(&text(&reversed)) {
                    found += 1;
                    if found == count {
                        return Ok(offset + 1);
                    }
                }
                reversed.clear();
            }
            reversed.push(byte);
        }
    }
    Ok(0)
}

/// Preserve an incomplete record between polls, including split UTF-8 bytes.
pub fn read_records(
    reader: &mut impl BufRead,
    pending: &mut Vec<u8>,
    mut emit: impl FnMut(&str) -> io::Result<()>,
) -> io::Result<()> {
    while reader.read_until(b'\n', pending)? != 0 {
        if pending.ends_with(b"\n") {
            emit(&text(pending))?;
            pending.clear();
        }
    }
    Ok(())
}

fn replaced(previous: &Metadata, current: &Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (previous.dev(), previous.ino()) != (current.dev(), current.ino())
    }
    #[cfg(not(unix))]
    {
        let _ = (previous, current);
        // ponytail: portable std metadata has no stable file ID; add a platform
        // file-ID API when rotation support outside Unix is needed.
        false
    }
}

pub fn follow(
    path: &Path,
    file: File,
    mut pending: Vec<u8>,
    mut emit: impl FnMut(&str) -> io::Result<()>,
) -> io::Result<()> {
    let mut reader = BufReader::new(file);
    loop {
        let metadata = reader.get_ref().metadata()?;
        if metadata.len() < reader.stream_position()? {
            reader.seek(SeekFrom::Start(0))?;
            pending.clear();
        }
        read_records(&mut reader, &mut pending, &mut emit)?;
        match fs::metadata(path) {
            Ok(current) if replaced(&metadata, &current) => match File::open(path) {
                Ok(file) => {
                    if !file.metadata()?.is_file() {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "expected a regular log file",
                        ));
                    }
                    reader = BufReader::new(file);
                    pending.clear();
                    continue;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            },
            Ok(_) => {}
            // Keep the old handle while a rotating file is temporarily absent.
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        thread::sleep(Duration::from_millis(200));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn backwards_scan_matches_forward_reference_at_block_boundaries() {
        let mut cases = vec![
            String::new(),
            "\n".into(),
            "\n\n".into(),
            "first\nlast".into(),
            "first\r\nlast\r\n".into(),
            "last\r".into(),
        ];
        for length in [BLOCK_SIZE - 1, BLOCK_SIZE, BLOCK_SIZE + 1, BLOCK_SIZE * 3] {
            cases.push(format!("first\n{}\nmatch\nlast", "ž".repeat(length)));
        }
        for input in cases {
            for count in [0, 1, 2, 10, usize::MAX] {
                for filtered in [false, true] {
                    let matches = |line: &str| !filtered || line.contains("match");
                    let all: Vec<_> = input.lines().filter(|line| matches(line)).collect();
                    let expected = &all[all.len().saturating_sub(count)..];
                    let start = find_start(
                        &mut Cursor::new(input.as_bytes()),
                        input.len() as u64,
                        count,
                        matches,
                    )
                    .unwrap();
                    let actual: Vec<_> = input[start as usize..]
                        .lines()
                        .filter(|line| matches(line))
                        .collect();
                    assert_eq!(
                        actual,
                        expected,
                        "length={}, count={count}, filtered={filtered}",
                        input.len()
                    );
                }
            }
        }
    }

    #[test]
    fn recent_records_never_read_the_large_prefix() {
        struct GuardedFile(Cursor<Vec<u8>>);
        impl Read for GuardedFile {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                assert!(
                    self.0.position() >= self.0.get_ref().len() as u64 - BLOCK_SIZE as u64,
                    "reading old data to retrieve recent records"
                );
                self.0.read(buffer)
            }
        }
        impl Seek for GuardedFile {
            fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
                self.0.seek(position)
            }
        }
        let mut bytes = vec![b'x'; BLOCK_SIZE * 4];
        bytes.extend_from_slice(b"\n[ERROR] old\n[INFO] last\n");
        let end = bytes.len() as u64;
        let mut reader = GuardedFile(Cursor::new(bytes));
        assert_eq!(find_start(&mut reader, end, 1, |_| true).unwrap(), end - 12);
        assert_eq!(find_start(&mut reader, end, 0, |_| true).unwrap(), end);
    }

    #[test]
    fn incomplete_utf8_records_are_emitted_only_once_when_completed() {
        let mut pending = Vec::new();
        let mut lines = Vec::new();
        for bytes in [
            &b"[INFO] \xc5"[..],
            &b"\xbe partial"[..],
            &b"\r\nnext\n"[..],
        ] {
            read_records(&mut Cursor::new(bytes), &mut pending, |line| {
                lines.push(line.to_owned());
                Ok(())
            })
            .unwrap();
        }
        assert_eq!(lines, ["[INFO] ž partial", "next"]);
        assert!(pending.is_empty());
    }
}
