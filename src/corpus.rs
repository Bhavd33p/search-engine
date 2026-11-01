//! The corpus format shared by the crawler and the indexer: one JSON document per line.
//!
//! ```json
//! {"id":"https://example.org/a","url":"https://example.org/a","title":"A","body":"…"}
//! ```

use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    /// Stable external identifier (a URL for crawled pages, a number for Cranfield).
    pub id: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub title: String,
    pub body: String,
}

#[derive(Debug, Error)]
pub enum CorpusError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("line {line}: {source}")]
    Parse { line: usize, source: serde_json::Error },
}

pub fn read_jsonl(path: &Path) -> Result<Vec<Document>, CorpusError> {
    let reader = BufReader::new(File::open(path)?);
    let mut documents = Vec::new();
    for (index, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let document = serde_json::from_str(&line).map_err(|source| CorpusError::Parse {
            line: index + 1,
            source,
        })?;
        documents.push(document);
    }
    Ok(documents)
}

pub struct JsonlWriter {
    out: BufWriter<File>,
}

impl JsonlWriter {
    pub fn create(path: &Path) -> Result<Self, CorpusError> {
        Ok(Self {
            out: BufWriter::new(File::create(path)?),
        })
    }

    pub fn write(&mut self, document: &Document) -> Result<(), CorpusError> {
        serde_json::to_writer(&mut self.out, document).map_err(std::io::Error::from)?;
        self.out.write_all(b"\n")?;
        Ok(())
    }

    pub fn finish(mut self) -> Result<(), CorpusError> {
        self.out.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_reads_back_documents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("corpus.jsonl");
        let docs = vec![
            Document {
                id: "1".into(),
                url: "https://a.example/".into(),
                title: "A".into(),
                body: "alpha".into(),
            },
            Document {
                id: "2".into(),
                url: String::new(),
                title: String::new(),
                body: "beta\nwith newline".into(),
            },
        ];
        let mut writer = JsonlWriter::create(&path).unwrap();
        for doc in &docs {
            writer.write(doc).unwrap();
        }
        writer.finish().unwrap();
        assert_eq!(read_jsonl(&path).unwrap(), docs);
    }

    #[test]
    fn reports_the_line_of_a_bad_record() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.jsonl");
        std::fs::write(&path, "{\"id\":\"1\",\"body\":\"ok\"}\n\nnot json\n").unwrap();
        let err = read_jsonl(&path).unwrap_err();
        assert!(matches!(err, CorpusError::Parse { line: 3, .. }), "{err}");
    }
}
