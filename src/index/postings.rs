//! Positional posting lists and their on-disk byte format.
//!
//! A posting list records, for one term, every document that contains it and
//! the word positions inside each document:
//!
//! ```text
//! "fox" -> [ (doc 3, [3]), (doc 7, [1, 5]), (doc 300, [2]) ]
//! ```
//!
//! [`Codec::VByte`] stores *gaps* instead of absolute values — doc ids are
//! delta-encoded against the previous doc id, positions against the previous
//! position in the same document — and writes every number as a VByte.
//! Sorted lists have small gaps, and small numbers fit in one byte:
//!
//! ```text
//! df=3 | gap 3, tf 1, pos 3 | gap 4, tf 2, pos 1, +4 | gap 293, tf 1, pos 2
//! 03   | 03     01    03    | 04     02    01     04 | A5 02    01    02     = 12 bytes
//! ```
//!
//! [`Codec::FixedU32`] is the uncompressed baseline used by the benchmarks:
//! the same fields as absolute little-endian `u32`s (44 bytes for the example).

use serde::{Deserialize, Serialize};

use super::vbyte::{self, DecodeError};

/// One document's entry in a posting list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Posting {
    pub doc_id: u32,
    /// Word positions of the term in the document, strictly increasing.
    pub positions: Vec<u32>,
}

impl Posting {
    /// How many times the term occurs in this document.
    pub fn term_frequency(&self) -> u32 {
        self.positions.len() as u32
    }
}

/// All postings for one term, sorted by doc id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PostingList {
    postings: Vec<Posting>,
}

impl PostingList {
    /// `postings` must be sorted by doc id with sorted positions; the index
    /// builder guarantees this by adding documents in id order.
    pub fn new(postings: Vec<Posting>) -> Self {
        debug_assert!(postings.windows(2).all(|w| w[0].doc_id < w[1].doc_id));
        debug_assert!(postings.iter().all(|p| p.positions.windows(2).all(|w| w[0] < w[1])));
        Self { postings }
    }

    pub fn postings(&self) -> &[Posting] {
        &self.postings
    }

    /// Number of documents containing the term (`df`).
    pub fn document_frequency(&self) -> u32 {
        self.postings.len() as u32
    }

    pub fn doc_ids(&self) -> Vec<u32> {
        self.postings.iter().map(|p| p.doc_id).collect()
    }

    /// Binary search for one document's posting.
    pub fn get(&self, doc_id: u32) -> Option<&Posting> {
        self.postings
            .binary_search_by_key(&doc_id, |p| p.doc_id)
            .ok()
            .map(|i| &self.postings[i])
    }

    /// Total number of positions across all documents.
    pub fn position_count(&self) -> usize {
        self.postings.iter().map(|p| p.positions.len()).sum()
    }
}

/// How posting lists are serialised. Stored in the index metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Codec {
    /// Delta gaps + variable-byte integers (the default).
    #[value(name = "vbyte")]
    VByte,
    /// Absolute values as fixed 4-byte integers (uncompressed baseline).
    #[value(name = "fixed32")]
    FixedU32,
}

pub fn encode(list: &PostingList, codec: Codec) -> Vec<u8> {
    match codec {
        Codec::VByte => encode_vbyte(list),
        Codec::FixedU32 => encode_fixed_u32(list),
    }
}

pub fn decode(bytes: &[u8], codec: Codec) -> Result<PostingList, DecodeError> {
    match codec {
        Codec::VByte => decode_vbyte(bytes),
        Codec::FixedU32 => decode_fixed_u32(bytes),
    }
}

/// Size the list would take with [`Codec::FixedU32`], without encoding it.
pub fn fixed_u32_len(list: &PostingList) -> usize {
    4 * (1 + 2 * list.postings.len() + list.position_count())
}

fn encode_vbyte(list: &PostingList) -> Vec<u8> {
    let mut out = Vec::new();
    vbyte::encode_u32(list.document_frequency(), &mut out);
    let mut previous_doc = 0;
    for posting in &list.postings {
        vbyte::encode_u32(posting.doc_id - previous_doc, &mut out);
        previous_doc = posting.doc_id;
        vbyte::encode_u32(posting.term_frequency(), &mut out);
        let mut previous_position = 0;
        for &position in &posting.positions {
            vbyte::encode_u32(position - previous_position, &mut out);
            previous_position = position;
        }
    }
    out
}

fn decode_vbyte(bytes: &[u8]) -> Result<PostingList, DecodeError> {
    let mut reader = vbyte::Reader::new(bytes);
    let document_frequency = reader.read_u32()?;
    // Cap the pre-allocation: a corrupted df must not trigger a huge allocation.
    let mut postings = Vec::with_capacity((document_frequency as usize).min(bytes.len()));
    let mut doc_id: u32 = 0;
    for _ in 0..document_frequency {
        doc_id = add_gap(doc_id, reader.read_u32()?)?;
        let term_frequency = reader.read_u32()?;
        let mut positions = Vec::with_capacity((term_frequency as usize).min(bytes.len()));
        let mut position: u32 = 0;
        for _ in 0..term_frequency {
            position = add_gap(position, reader.read_u32()?)?;
            positions.push(position);
        }
        postings.push(Posting { doc_id, positions });
    }
    reader.finish()?;
    Ok(PostingList { postings })
}

fn add_gap(previous: u32, gap: u32) -> Result<u32, DecodeError> {
    previous.checked_add(gap).ok_or(DecodeError::GapOverflow)
}

fn encode_fixed_u32(list: &PostingList) -> Vec<u8> {
    let mut out = Vec::with_capacity(fixed_u32_len(list));
    out.extend_from_slice(&list.document_frequency().to_le_bytes());
    for posting in &list.postings {
        out.extend_from_slice(&posting.doc_id.to_le_bytes());
        out.extend_from_slice(&posting.term_frequency().to_le_bytes());
        for &position in &posting.positions {
            out.extend_from_slice(&position.to_le_bytes());
        }
    }
    out
}

fn decode_fixed_u32(bytes: &[u8]) -> Result<PostingList, DecodeError> {
    let mut offset = 0;
    let mut next = || -> Result<u32, DecodeError> {
        let chunk = bytes
            .get(offset..offset + 4)
            .ok_or(DecodeError::UnexpectedEof(offset))?;
        offset += 4;
        Ok(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
    };
    let document_frequency = next()?;
    let mut postings = Vec::with_capacity((document_frequency as usize).min(bytes.len() / 4));
    for _ in 0..document_frequency {
        let doc_id = next()?;
        let term_frequency = next()?;
        let positions = (0..term_frequency).map(|_| next()).collect::<Result<Vec<_>, _>>()?;
        postings.push(Posting { doc_id, positions });
    }
    match bytes.len() - offset {
        0 => Ok(PostingList { postings }),
        remaining => Err(DecodeError::TrailingBytes(remaining)),
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn fox() -> PostingList {
        PostingList::new(vec![
            Posting {
                doc_id: 3,
                positions: vec![3],
            },
            Posting {
                doc_id: 7,
                positions: vec![1, 5],
            },
            Posting {
                doc_id: 300,
                positions: vec![2],
            },
        ])
    }

    #[test]
    fn vbyte_encodes_the_documented_example() {
        let bytes = encode(&fox(), Codec::VByte);
        assert_eq!(
            bytes,
            [0x03, 0x03, 0x01, 0x03, 0x04, 0x02, 0x01, 0x04, 0xA5, 0x02, 0x01, 0x02]
        );
    }

    #[test]
    fn fixed_u32_baseline_uses_four_bytes_per_number() {
        let bytes = encode(&fox(), Codec::FixedU32);
        assert_eq!(bytes.len(), 44);
        assert_eq!(bytes.len(), fixed_u32_len(&fox()));
        assert_eq!(&bytes[..8], &[3, 0, 0, 0, 3, 0, 0, 0]);
    }

    #[test]
    fn both_codecs_round_trip() {
        for codec in [Codec::VByte, Codec::FixedU32] {
            assert_eq!(decode(&encode(&fox(), codec), codec), Ok(fox()));
        }
    }

    #[test]
    fn empty_list_round_trips() {
        let empty = PostingList::default();
        assert_eq!(decode(&encode(&empty, Codec::VByte), Codec::VByte), Ok(empty));
    }

    #[test]
    fn truncated_blob_is_an_error() {
        let bytes = encode(&fox(), Codec::VByte);
        assert!(decode(&bytes[..bytes.len() - 1], Codec::VByte).is_err());
        let bytes = encode(&fox(), Codec::FixedU32);
        assert!(decode(&bytes[..bytes.len() - 1], Codec::FixedU32).is_err());
    }

    #[test]
    fn get_finds_postings_by_doc_id() {
        let list = fox();
        assert_eq!(list.get(7).map(Posting::term_frequency), Some(2));
        assert_eq!(list.get(8), None);
        assert_eq!(list.doc_ids(), [3, 7, 300]);
        assert_eq!(list.position_count(), 4);
    }

    fn arbitrary_posting_list() -> impl Strategy<Value = PostingList> {
        let posting = (1u32..1_000, proptest::collection::btree_set(0u32..100_000, 1..20));
        proptest::collection::vec(posting, 0..50).prop_map(|entries| {
            let mut doc_id = 0;
            let postings = entries
                .into_iter()
                .map(|(gap, positions)| {
                    doc_id += gap;
                    Posting {
                        doc_id,
                        positions: positions.into_iter().collect(),
                    }
                })
                .collect();
            PostingList::new(postings)
        })
    }

    proptest! {
        #[test]
        fn any_posting_list_round_trips(list in arbitrary_posting_list()) {
            for codec in [Codec::VByte, Codec::FixedU32] {
                prop_assert_eq!(decode(&encode(&list, codec), codec), Ok(list.clone()));
            }
        }

        #[test]
        fn vbyte_is_never_larger_than_fixed_u32(list in arbitrary_posting_list()) {
            prop_assert!(encode(&list, Codec::VByte).len() <= fixed_u32_len(&list));
        }
    }
}
