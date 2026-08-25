pub use crate::error::SessionLogError;
use crate::model::{SessionEvent, SessionMeta};
use serde_json::Error as JsonError;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

#[derive(Debug, Clone)]
pub struct SessionLine {
    pub line_number: usize,
    pub bytes_read: usize,
    pub raw: String,
}

#[derive(Debug)]
pub enum SessionLogEntry {
    Persisted { line: SessionLine, event: Box<SessionEvent> },
    Transient { line: SessionLine },
    Malformed { line: SessionLine, error: JsonError },
}

impl SessionLogEntry {
    pub fn line(&self) -> &SessionLine {
        match self {
            Self::Persisted { line, .. } | Self::Transient { line } | Self::Malformed { line, .. } => line,
        }
    }
}

pub struct SessionLog<T: BufRead> {
    reader: T,
    pub meta: SessionMeta,
    line_number: usize,
}

impl SessionLog<BufReader<File>> {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, SessionLogError> {
        Self::from_reader(BufReader::new(File::open(path.as_ref())?))
    }
}

impl<T: BufRead> SessionLog<T> {
    pub fn from_reader(mut reader: T) -> Result<Self, SessionLogError> {
        let mut line = String::new();
        let mut line_number = 0;
        loop {
            line.clear();
            if reader.read_line(&mut line)? == 0 {
                return Err(SessionLogError::MissingMetadata);
            }
            line_number += 1;
            if !line.trim().is_empty() {
                break;
            }
        }
        let meta = serde_json::from_str(line.trim())
            .map_err(|source| SessionLogError::InvalidMetadata { line_number, source })?;
        Ok(Self { reader, meta, line_number })
    }

    pub fn next_entry(&mut self) -> std::io::Result<Option<SessionLogEntry>> {
        let Some(line) = self.next_line()? else {
            return Ok(None);
        };
        let entry = match serde_json::from_str::<SessionEvent>(&line.raw) {
            Ok(event) if event.is_persisted() => SessionLogEntry::Persisted { line, event: Box::new(event) },
            Ok(_) => SessionLogEntry::Transient { line },
            Err(error) => SessionLogEntry::Malformed { line, error },
        };
        Ok(Some(entry))
    }

    fn next_line(&mut self) -> std::io::Result<Option<SessionLine>> {
        let mut line = String::new();
        loop {
            line.clear();
            let bytes_read = self.reader.read_line(&mut line)?;
            if bytes_read == 0 {
                return Ok(None);
            }
            self.line_number += 1;
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                return Ok(Some(SessionLine { line_number: self.line_number, bytes_read, raw: trimmed.to_string() }));
            }
        }
    }
}
