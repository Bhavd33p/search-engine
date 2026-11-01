//! Persists the inverted index in RocksDB, an embedded LSM-tree key-value store.
//!
//! Three column families (separate key spaces with their own tuning):
//!
//! | column family | key                    | value                                  |
//! |---------------|------------------------|----------------------------------------|
//! | `postings`    | term (UTF-8)           | encoded posting list                   |
//! | `docs`        | doc id (big-endian u32) | stored fields as JSON                 |
//! | `meta`        | `stats`, `doc_lengths` | collection statistics and doc lengths  |
//!
//! Doc ids are big-endian so RocksDB's byte-wise key order matches numeric order.
//! RocksDB block compression is switched off so the on-disk size reflects our
//! own posting encoding, not Snappy/LZ4.

use std::path::{Path, PathBuf};

use rocksdb::{BlockBasedOptions, ColumnFamily, ColumnFamilyDescriptor, DB, DBCompressionType, Options, WriteBatch};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::builder::InMemoryIndex;
use super::postings::{self, Codec, PostingList};
use super::vbyte::{self, DecodeError};
use crate::corpus::Document;

pub const CF_POSTINGS: &str = "postings";
pub const CF_DOCS: &str = "docs";
pub const CF_META: &str = "meta";
const COLUMN_FAMILIES: [&str; 3] = [CF_POSTINGS, CF_DOCS, CF_META];

const META_STATS: &[u8] = b"stats";
const META_DOC_LENGTHS: &[u8] = b"doc_lengths";

/// Bumped whenever the on-disk layout changes.
pub const FORMAT_VERSION: u32 = 1;

/// Postings written per RocksDB write batch.
const WRITE_BATCH_SIZE: usize = 10_000;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("rocksdb: {0}")]
    Rocks(#[from] rocksdb::Error),
    #[error("corrupt posting list: {0}")]
    Decode(#[from] DecodeError),
    #[error("corrupt stored value: {0}")]
    Json(#[from] serde_json::Error),
    #[error("index directory {0} already exists")]
    AlreadyExists(PathBuf),
    #[error("index is missing {0}")]
    Missing(&'static str),
    #[error("index format version {found} is not supported (expected {FORMAT_VERSION})")]
    UnsupportedFormat { found: u32 },
}

/// Collection-wide statistics, stored as JSON under `meta/stats`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexStats {
    pub format_version: u32,
    pub codec: Codec,
    pub num_docs: u32,
    pub num_terms: u64,
    /// Sum of all document lengths, in indexed terms.
    pub total_doc_len: u64,
    /// Fingerprint of the index contents; changes whenever the index does.
    /// Used to namespace cached query results.
    pub index_version: String,
}

impl IndexStats {
    pub fn avg_doc_len(&self) -> f64 {
        if self.num_docs == 0 {
            0.0
        } else {
            self.total_doc_len as f64 / self.num_docs as f64
        }
    }
}

/// What [`write_index`] wrote, for reporting.
#[derive(Debug, Clone, Default, Serialize)]
pub struct WriteReport {
    pub num_terms: u64,
    pub num_postings: u64,
    pub num_positions: u64,
    /// Bytes of encoded posting lists with the chosen codec.
    pub postings_bytes: u64,
    /// Bytes the same postings would take as fixed-width u32s.
    pub fixed_u32_postings_bytes: u64,
    /// Bytes of stored document JSON.
    pub docs_bytes: u64,
}

/// Writes `index` to a new RocksDB database at `path`.
pub fn write_index(path: &Path, index: &InMemoryIndex, codec: Codec) -> Result<(IndexStats, WriteReport), StoreError> {
    if path.exists() {
        return Err(StoreError::AlreadyExists(path.to_path_buf()));
    }
    let db = open_for_write(path)?;
    let postings_cf = cf(&db, CF_POSTINGS)?;
    let docs_cf = cf(&db, CF_DOCS)?;
    let meta_cf = cf(&db, CF_META)?;

    // The index is rebuilt from the corpus if anything fails, so the write-ahead
    // log only costs time here. Everything is flushed to SST files at the end.
    let mut write_options = rocksdb::WriteOptions::default();
    write_options.disable_wal(true);

    let mut report = WriteReport {
        num_terms: index.terms.len() as u64,
        ..WriteReport::default()
    };
    let mut fingerprint = Fnv1a::new();
    fingerprint.write(&FORMAT_VERSION.to_le_bytes());

    for chunk in index.terms.chunks(WRITE_BATCH_SIZE) {
        let mut batch = WriteBatch::default();
        for (term, list) in chunk {
            let bytes = postings::encode(list, codec);
            report.num_postings += list.postings().len() as u64;
            report.num_positions += list.position_count() as u64;
            report.postings_bytes += bytes.len() as u64;
            report.fixed_u32_postings_bytes += postings::fixed_u32_len(list) as u64;
            fingerprint.write(term.as_bytes());
            fingerprint.write(&bytes);
            batch.put_cf(postings_cf, term.as_bytes(), &bytes);
        }
        db.write_opt(batch, &write_options)?;
    }

    for (chunk_index, chunk) in index.documents.chunks(WRITE_BATCH_SIZE).enumerate() {
        let mut batch = WriteBatch::default();
        for (offset, document) in chunk.iter().enumerate() {
            let doc_id = (chunk_index * WRITE_BATCH_SIZE + offset) as u32;
            let json = serde_json::to_vec(document)?;
            report.docs_bytes += json.len() as u64;
            fingerprint.write(&json);
            batch.put_cf(docs_cf, doc_id.to_be_bytes(), json);
        }
        db.write_opt(batch, &write_options)?;
    }

    let stats = IndexStats {
        format_version: FORMAT_VERSION,
        codec,
        num_docs: index.num_docs(),
        num_terms: index.terms.len() as u64,
        total_doc_len: index.doc_lengths.iter().map(|&l| u64::from(l)).sum(),
        index_version: format!("{:016x}", fingerprint.finish()),
    };
    let mut doc_lengths = Vec::new();
    for &length in &index.doc_lengths {
        vbyte::encode_u32(length, &mut doc_lengths);
    }
    let mut batch = WriteBatch::default();
    batch.put_cf(meta_cf, META_STATS, serde_json::to_vec(&stats)?);
    batch.put_cf(meta_cf, META_DOC_LENGTHS, doc_lengths);
    db.write_opt(batch, &write_options)?;

    for name in COLUMN_FAMILIES {
        let handle = cf(&db, name)?;
        db.flush_cf(handle)?;
        // Merge the flushed files into one sorted run so reads touch fewer files.
        db.compact_range_cf(handle, None::<&[u8]>, None::<&[u8]>);
    }
    Ok((stats, report))
}

/// Read-only handle on a written index. Several processes may open the same
/// index at once, which the cold-latency benchmark relies on.
pub struct IndexReader {
    db: DB,
    stats: IndexStats,
    doc_lengths: Vec<u32>,
}

impl IndexReader {
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let descriptors = COLUMN_FAMILIES.map(|name| ColumnFamilyDescriptor::new(name, column_family_options(name)));
        let db = DB::open_cf_descriptors_read_only(&Options::default(), path, descriptors, false)?;

        let stats_bytes = db
            .get_cf(cf(&db, CF_META)?, META_STATS)?
            .ok_or(StoreError::Missing("meta/stats"))?;
        let stats: IndexStats = serde_json::from_slice(&stats_bytes)?;
        if stats.format_version != FORMAT_VERSION {
            return Err(StoreError::UnsupportedFormat {
                found: stats.format_version,
            });
        }

        let lengths_bytes = db
            .get_cf(cf(&db, CF_META)?, META_DOC_LENGTHS)?
            .ok_or(StoreError::Missing("meta/doc_lengths"))?;
        let mut reader = vbyte::Reader::new(&lengths_bytes);
        let doc_lengths = (0..stats.num_docs)
            .map(|_| reader.read_u32())
            .collect::<Result<Vec<_>, _>>()?;
        reader.finish()?;

        Ok(Self { db, stats, doc_lengths })
    }

    pub fn stats(&self) -> &IndexStats {
        &self.stats
    }

    pub fn doc_length(&self, doc_id: u32) -> u32 {
        self.doc_lengths.get(doc_id as usize).copied().unwrap_or(0)
    }

    /// Reads and decodes one term's posting list; `None` if the term is not indexed.
    pub fn postings(&self, term: &str) -> Result<Option<PostingList>, StoreError> {
        match self.db.get_pinned_cf(cf(&self.db, CF_POSTINGS)?, term.as_bytes())? {
            Some(bytes) => Ok(Some(postings::decode(&bytes, self.stats.codec)?)),
            None => Ok(None),
        }
    }

    pub fn document(&self, doc_id: u32) -> Result<Option<Document>, StoreError> {
        match self.db.get_pinned_cf(cf(&self.db, CF_DOCS)?, doc_id.to_be_bytes())? {
            Some(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            None => Ok(None),
        }
    }

    /// Size of each column family's SST files on disk.
    pub fn column_family_sizes(&self) -> Result<Vec<(&'static str, u64)>, StoreError> {
        COLUMN_FAMILIES
            .iter()
            .map(|&name| {
                let size = self
                    .db
                    .property_int_value_cf(cf(&self.db, name)?, "rocksdb.total-sst-files-size")?;
                Ok((name, size.unwrap_or(0)))
            })
            .collect()
    }
}

fn open_for_write(path: &Path) -> Result<DB, StoreError> {
    let mut options = Options::default();
    options.create_if_missing(true);
    options.create_missing_column_families(true);
    options.set_compression_type(DBCompressionType::None);
    let descriptors = COLUMN_FAMILIES.map(|name| ColumnFamilyDescriptor::new(name, column_family_options(name)));
    Ok(DB::open_cf_descriptors(&options, path, descriptors)?)
}

fn column_family_options(name: &str) -> Options {
    let mut options = Options::default();
    options.set_compression_type(DBCompressionType::None);
    options.set_bottommost_compression_type(DBCompressionType::None);
    if name == CF_POSTINGS {
        // Bloom filters let lookups for terms that are not in the index skip disk reads.
        let mut table = BlockBasedOptions::default();
        table.set_bloom_filter(10.0, false);
        options.set_block_based_table_factory(&table);
    }
    options
}

fn cf<'a>(db: &'a DB, name: &'static str) -> Result<&'a ColumnFamily, StoreError> {
    db.cf_handle(name).ok_or(StoreError::Missing(name))
}

/// 64-bit FNV-1a: a tiny, stable, non-cryptographic hash for the index fingerprint.
struct Fnv1a(u64);

impl Fnv1a {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::Analyzer;

    fn sample_index() -> InMemoryIndex {
        let docs = [
            ("1", "Ownership", "Each value in Rust has an owner."),
            ("2", "Borrowing", "References borrow values without taking ownership."),
            ("3", "Lifetimes", "Lifetimes ensure references are valid."),
        ];
        let documents = docs.map(|(id, title, body)| Document {
            id: id.into(),
            url: format!("https://example.org/{id}"),
            title: title.into(),
            body: body.into(),
        });
        InMemoryIndex::build(&Analyzer::new(), documents)
    }

    #[test]
    fn written_index_reads_back_identically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index");
        let index = sample_index();
        for codec in [Codec::VByte, Codec::FixedU32] {
            let path = path.join(format!("{codec:?}"));
            let (stats, report) = write_index(&path, &index, codec).unwrap();
            assert_eq!(stats.num_docs, 3);
            assert_eq!(report.num_terms, index.terms.len() as u64);

            let reader = IndexReader::open(&path).unwrap();
            assert_eq!(reader.stats(), &stats);
            for (term, list) in &index.terms {
                assert_eq!(reader.postings(term).unwrap().as_ref(), Some(list), "term {term}");
            }
            assert_eq!(reader.postings("absent").unwrap(), None);
            for (doc_id, document) in index.documents.iter().enumerate() {
                assert_eq!(reader.document(doc_id as u32).unwrap().as_ref(), Some(document));
                assert_eq!(reader.doc_length(doc_id as u32), index.doc_lengths[doc_id]);
            }
        }
    }

    #[test]
    fn vbyte_postings_are_smaller_than_fixed_width() {
        let dir = tempfile::tempdir().unwrap();
        let (_, report) = write_index(&dir.path().join("index"), &sample_index(), Codec::VByte).unwrap();
        assert!(report.postings_bytes < report.fixed_u32_postings_bytes);
    }

    #[test]
    fn index_version_is_deterministic_and_content_sensitive() {
        let dir = tempfile::tempdir().unwrap();
        let (a, _) = write_index(&dir.path().join("a"), &sample_index(), Codec::VByte).unwrap();
        let (b, _) = write_index(&dir.path().join("b"), &sample_index(), Codec::VByte).unwrap();
        assert_eq!(a.index_version, b.index_version);

        let mut changed = sample_index();
        changed.documents[0].title.push('!');
        let (c, _) = write_index(&dir.path().join("c"), &changed, Codec::VByte).unwrap();
        assert_ne!(a.index_version, c.index_version);
    }

    #[test]
    fn refuses_to_overwrite_an_existing_directory() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            write_index(dir.path(), &sample_index(), Codec::VByte),
            Err(StoreError::AlreadyExists(_))
        ));
    }
}
