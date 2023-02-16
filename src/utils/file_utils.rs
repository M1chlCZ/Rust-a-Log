use std::fs::File;
use std::io::{Error, ErrorKind};

    pub fn open_file(path: &str) -> Result<File, Error> {
        match File::open(path) {
            Ok(file) => Ok(file),
            Err(_) => Err(Error::new(
                ErrorKind::Other,
                "Err: 69 | No such file - wrong file path",
            )),
        }
    }
