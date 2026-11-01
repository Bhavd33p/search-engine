//! Loader for the Cranfield collection: 1,400 aerodynamics abstracts, 225
//! queries and human relevance judgements — the classic labelled test set for
//! measuring retrieval quality.
//!
//! Files (SMART format, from the University of Glasgow IR test collections):
//! * `cran.all.1400` — documents: `.I id`, `.T` title, `.A` authors, `.B` source, `.W` abstract;
//! * `cran.qry` — queries: `.I id`, `.W` text;
//! * `cranqrel` — judgements: `query doc grade` per line.
//!
//! Two quirks worth knowing:
//! * query ids in `cran.qry` are not contiguous (1, 2, 4, 8, …), but `cranqrel`
//!   numbers queries 1..=225 by their *position* in `cran.qry`, so queries are
//!   renumbered by position here;
//! * grades run 1 (complete answer) to 4 (minimum interest). Each query also has
//!   one undocumented `-1` row, which is ignored.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use thiserror::Error;

use crate::corpus::Document;

#[derive(Debug, Error)]
pub enum CranfieldError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{file} line {line}: {message}")]
    Format {
        file: &'static str,
        line: usize,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CranfieldQuery {
    /// Position in `cran.qry`, 1-based — the id `cranqrel` uses.
    pub id: u32,
    pub text: String,
}

/// Relevance grades per query: query id → (document id → grade 1..=4).
pub type Judgements = HashMap<u32, HashMap<String, u8>>;

/// One `.I` record: its id and its fields by marker letter.
struct Record {
    id: String,
    fields: HashMap<char, String>,
}

fn parse_records(text: &str, file: &'static str) -> Result<Vec<Record>, CranfieldError> {
    let mut records: Vec<Record> = Vec::new();
    let mut field: Option<char> = None;
    for (index, line) in text.lines().enumerate() {
        if let Some(id) = line.strip_prefix(".I ") {
            records.push(Record {
                id: id.trim().to_string(),
                fields: HashMap::new(),
            });
            field = None;
        } else if let Some(marker) = line
            .strip_prefix('.')
            .filter(|rest| rest.len() == 1)
            .and_then(|m| m.chars().next())
        {
            field = Some(marker);
        } else {
            let (Some(record), Some(marker)) = (records.last_mut(), field) else {
                return Err(CranfieldError::Format {
                    file,
                    line: index + 1,
                    message: "text outside a record".into(),
                });
            };
            let value = record.fields.entry(marker).or_default();
            if !value.is_empty() {
                value.push(' ');
            }
            value.push_str(line.trim());
        }
    }
    Ok(records)
}

pub fn read_documents(path: &Path) -> Result<Vec<Document>, CranfieldError> {
    let records = parse_records(&fs::read_to_string(path)?, "cran.all.1400")?;
    Ok(records
        .into_iter()
        .map(|mut record| Document {
            url: String::new(),
            title: record.fields.remove(&'T').unwrap_or_default(),
            // The abstract (.W) repeats the title as its first sentence.
            body: record.fields.remove(&'W').unwrap_or_default(),
            id: record.id,
        })
        .collect())
}

pub fn read_queries(path: &Path) -> Result<Vec<CranfieldQuery>, CranfieldError> {
    let records = parse_records(&fs::read_to_string(path)?, "cran.qry")?;
    Ok(records
        .into_iter()
        .enumerate()
        .map(|(position, mut record)| CranfieldQuery {
            id: position as u32 + 1,
            text: record.fields.remove(&'W').unwrap_or_default(),
        })
        .collect())
}

pub fn read_judgements(path: &Path) -> Result<Judgements, CranfieldError> {
    let mut judgements: Judgements = HashMap::new();
    for (index, line) in fs::read_to_string(path)?.lines().enumerate() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.is_empty() {
            continue;
        }
        let bad = |message: &str| CranfieldError::Format {
            file: "cranqrel",
            line: index + 1,
            message: message.into(),
        };
        let [query, doc, grade] = fields[..] else {
            return Err(bad("expected `query doc grade`"));
        };
        let query: u32 = query.parse().map_err(|_| bad("query id is not a number"))?;
        let grade: i32 = grade.parse().map_err(|_| bad("grade is not a number"))?;
        if let Ok(grade @ 1..=4) = u8::try_from(grade) {
            judgements.entry(query).or_default().insert(doc.to_string(), grade);
        }
    }
    Ok(judgements)
}

/// Graded gain for nDCG: grade 1 (complete answer) is worth 4, grade 4 is worth 1.
pub fn gain(grade: u8) -> f64 {
    f64::from(5u8.saturating_sub(grade))
}

/// Relevant documents for a query (every judged grade 1–4).
pub fn relevant_docs(judgements: &Judgements, query: u32) -> HashSet<String> {
    judgements
        .get(&query)
        .map(|docs| docs.keys().cloned().collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, contents: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn parses_documents() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "docs",
            ".I 1\n.T\nexperimental investigation of the aerodynamics of a\nwing in a slipstream .\n.A\nbrenckman,m.\n.B\nj. ae. scs. 25, 1958, 324.\n.W\nexperimental investigation\nof a wing .\n.I 2\n.T\n.A\n.B\n.W\n",
        );
        let docs = read_documents(&path).unwrap();
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[0].id, "1");
        assert_eq!(
            docs[0].title,
            "experimental investigation of the aerodynamics of a wing in a slipstream ."
        );
        assert_eq!(docs[0].body, "experimental investigation of a wing .");
        assert_eq!(docs[1].body, "");
    }

    #[test]
    fn numbers_queries_by_position() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "qry",
            ".I 001\n.W\nfirst query\ntext .\n.I 004\n.W\nsecond query .\n",
        );
        let queries = read_queries(&path).unwrap();
        assert_eq!(
            queries,
            [
                CranfieldQuery {
                    id: 1,
                    text: "first query text .".into()
                },
                CranfieldQuery {
                    id: 2,
                    text: "second query .".into()
                },
            ]
        );
    }

    #[test]
    fn keeps_grades_one_to_four_and_drops_minus_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "qrel", "1 184 2 \n1 29 4\n1 486 -1\n2 12 1\n");
        let judgements = read_judgements(&path).unwrap();
        assert_eq!(judgements[&1].len(), 2);
        assert_eq!(judgements[&1]["184"], 2);
        assert!(!judgements[&1].contains_key("486"));
        assert_eq!(relevant_docs(&judgements, 2), HashSet::from(["12".to_string()]));
        assert_eq!(gain(1), 4.0);
        assert_eq!(gain(4), 1.0);
    }

    #[test]
    fn malformed_judgement_reports_its_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "qrel", "1 2 3\nbroken\n");
        assert!(matches!(
            read_judgements(&path),
            Err(CranfieldError::Format { line: 2, .. })
        ));
    }
}
