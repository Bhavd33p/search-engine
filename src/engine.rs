//! The query engine: turns a query into ranked, snippeted hits.
//!
//! A query's life:
//! 1. parse the text into a [`Query`] tree (or OR every word, in any-terms mode);
//! 2. analyse it, so words become index terms ([`AnalyzedQuery`]);
//! 3. evaluate the boolean/phrase structure to the set of matching doc ids;
//! 4. score each match with BM25 or TF-IDF, summing over the positive query terms;
//! 5. keep the k best with a min-heap, then load titles/URLs and build snippets.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::analysis::Analyzer;
use crate::cache::hot_terms::{HotTermCache, HotTermStats};
use crate::index::postings::PostingList;
use crate::index::store::{IndexReader, IndexStats, StoreError};
use crate::query::eval::{PostingSource, matching_docs};
use crate::query::{AnalyzedQuery, ParseError, parse};
use crate::rank::topk::{ScoredDoc, top_k};
use crate::rank::{Bm25, CollectionStats, DocTermStats, RankerKind, Scorer, TermStats, TfIdf};
use crate::snippet::make_snippet;

const SNIPPET_WORDS: usize = 30;
/// Longer queries are rejected before parsing; real queries are a few dozen bytes.
pub const MAX_QUERY_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy)]
pub struct EngineConfig {
    pub bm25: Bm25,
    /// Decoded posting lists kept in memory; 0 disables the hot-term cache.
    pub hot_term_capacity: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            bm25: Bm25::default(),
            hot_term_capacity: 10_000,
        }
    }
}

/// How query text is interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QueryMode {
    /// The query language: implicit AND, `OR`, `NOT`/`-`, parentheses, phrases.
    #[default]
    Boolean,
    /// Natural-language text: OR of every word, ranked. Used for Cranfield.
    AnyTerms,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hit {
    pub doc_id: u32,
    pub external_id: String,
    pub title: String,
    pub url: String,
    pub score: f64,
    pub snippet: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchResults {
    pub hits: Vec<Hit>,
    /// Documents that matched the query, before top-k.
    pub total_matches: u32,
}

#[derive(Debug, Error)]
pub enum SearchError {
    #[error("invalid query: {0}")]
    Parse(ParseError),
    #[error("query has no searchable terms (only stop words?)")]
    NoSearchableTerms,
    #[error(transparent)]
    Store(#[from] StoreError),
}

// Not `#[from]`: that would also make the ParseError the error's *source*, and
// error reports would print the same message twice.
impl From<ParseError> for SearchError {
    fn from(error: ParseError) -> Self {
        Self::Parse(error)
    }
}

pub struct Engine {
    reader: IndexReader,
    analyzer: Analyzer,
    hot_terms: HotTermCache,
    config: EngineConfig,
}

impl Engine {
    pub fn open(path: &Path, config: EngineConfig) -> Result<Self, StoreError> {
        Ok(Self {
            reader: IndexReader::open(path)?,
            analyzer: Analyzer::new(),
            hot_terms: HotTermCache::new(config.hot_term_capacity),
            config,
        })
    }

    pub fn stats(&self) -> &IndexStats {
        self.reader.stats()
    }

    pub fn reader(&self) -> &IndexReader {
        &self.reader
    }

    pub fn hot_term_stats(&self) -> HotTermStats {
        self.hot_terms.stats()
    }

    /// Parses and analyses query text.
    pub fn analyze(&self, text: &str, mode: QueryMode) -> Result<AnalyzedQuery, SearchError> {
        if text.len() > MAX_QUERY_BYTES {
            return Err(ParseError::TooLong { max: MAX_QUERY_BYTES }.into());
        }
        let analyzed = match mode {
            QueryMode::Boolean => AnalyzedQuery::analyze(&parse(text)?, &self.analyzer),
            QueryMode::AnyTerms => AnalyzedQuery::any_terms(text, &self.analyzer),
        };
        analyzed.ok_or(SearchError::NoSearchableTerms)
    }

    /// Identifies a ranker *and its parameters*, so cached results computed with
    /// different BM25 settings are never mixed up.
    pub fn ranker_key(&self, ranker: RankerKind) -> String {
        match ranker {
            RankerKind::Bm25 => format!("bm25(k1={},b={})", self.config.bm25.k1, self.config.bm25.b),
            RankerKind::TfIdf => "tfidf".to_string(),
        }
    }

    pub fn search(&self, query: &AnalyzedQuery, k: usize, ranker: RankerKind) -> Result<SearchResults, SearchError> {
        let source = QueryPostings::new(self);
        let ranked = self.rank_with(&source, query, k, ranker)?;
        let terms = query.scoring_terms();
        let mut hits = Vec::with_capacity(ranked.docs.len());
        for ScoredDoc { doc_id, score } in ranked.docs {
            let document = self
                .reader
                .document(doc_id)?
                .ok_or(StoreError::Missing("a stored document"))?;
            let anchor = self.first_body_match(&source, &terms, doc_id, &document.title)?;
            hits.push(Hit {
                doc_id,
                snippet: make_snippet(&self.analyzer, &document.body, &terms, anchor, SNIPPET_WORDS),
                external_id: document.id,
                title: document.title,
                url: document.url,
                score,
            });
        }
        Ok(SearchResults {
            hits,
            total_matches: ranked.total_matches,
        })
    }

    /// Steps 3–5 without loading stored fields: the ranked doc ids only.
    pub fn rank(&self, query: &AnalyzedQuery, k: usize, ranker: RankerKind) -> Result<RankedDocs, SearchError> {
        self.rank_with(&QueryPostings::new(self), query, k, ranker)
    }

    fn rank_with(
        &self,
        source: &QueryPostings<'_>,
        query: &AnalyzedQuery,
        k: usize,
        ranker: RankerKind,
    ) -> Result<RankedDocs, SearchError> {
        let candidates = matching_docs(query, source, self.stats().num_docs)?;
        let terms = query.scoring_terms();
        let scores = match ranker {
            RankerKind::Bm25 => self.score(&self.config.bm25, source, &candidates, &terms)?,
            RankerKind::TfIdf => self.score(&TfIdf, source, &candidates, &terms)?,
        };
        let scored = candidates
            .iter()
            .zip(scores)
            .map(|(&doc_id, score)| ScoredDoc { doc_id, score });
        Ok(RankedDocs {
            docs: top_k(scored, k),
            total_matches: candidates.len() as u32,
        })
    }

    /// Index of the first body word that matches a query term, read straight
    /// from the positional postings (already fetched for scoring).
    fn first_body_match(
        &self,
        source: &QueryPostings<'_>,
        terms: &BTreeMap<String, u32>,
        doc_id: u32,
        title: &str,
    ) -> Result<Option<usize>, StoreError> {
        let body_start = self.analyzer.body_start_position(title);
        let mut first: Option<u32> = None;
        for term in terms.keys() {
            let Some(list) = source.postings(term)? else {
                continue;
            };
            let Some(posting) = list.get(doc_id) else {
                continue;
            };
            // Positions are sorted: the first one at or after the body start is this term's earliest body match.
            let index = posting.positions.partition_point(|&p| p < body_start);
            if let Some(&position) = posting.positions.get(index) {
                first = Some(first.map_or(position, |current| current.min(position)));
            }
        }
        Ok(first.map(|position| (position - body_start) as usize))
    }

    /// Term-at-a-time scoring: for each query term, walk its posting list
    /// alongside the sorted candidates and add that term's score to each match.
    fn score<S: Scorer>(
        &self,
        scorer: &S,
        source: &QueryPostings<'_>,
        candidates: &[u32],
        terms: &BTreeMap<String, u32>,
    ) -> Result<Vec<f64>, StoreError> {
        let stats = self.stats();
        let collection = CollectionStats {
            num_docs: stats.num_docs,
            avg_doc_len: stats.avg_doc_len(),
        };
        let mut scores = vec![0.0; candidates.len()];
        for (term, &query_frequency) in terms {
            let Some(list) = source.postings(term)? else {
                continue;
            };
            let term_stats = TermStats {
                document_frequency: list.document_frequency(),
                query_frequency,
            };
            let mut slot = 0;
            for posting in list.postings() {
                // Both lists are sorted by doc id: advance through candidates in step.
                while slot < candidates.len() && candidates[slot] < posting.doc_id {
                    slot += 1;
                }
                if slot == candidates.len() {
                    break;
                }
                if candidates[slot] == posting.doc_id {
                    let doc = DocTermStats {
                        term_frequency: posting.term_frequency(),
                        doc_length: self.reader.doc_length(posting.doc_id),
                        doc_norm: self.reader.doc_norm(posting.doc_id),
                    };
                    scores[slot] += scorer.score(&collection, &term_stats, &doc);
                }
            }
        }
        Ok(scores)
    }

    fn load_postings(&self, term: &str) -> Result<Option<Arc<PostingList>>, StoreError> {
        self.hot_terms.get_or_load(term, || self.reader.postings(term))
    }
}

/// Ranked doc ids for one query.
#[derive(Debug, Clone)]
pub struct RankedDocs {
    pub docs: Vec<ScoredDoc>,
    pub total_matches: u32,
}

/// Fetches each term's postings at most once per query, even when the term is
/// used for both matching and scoring.
struct QueryPostings<'a> {
    engine: &'a Engine,
    fetched: RefCell<HashMap<String, Option<Arc<PostingList>>>>,
}

impl<'a> QueryPostings<'a> {
    fn new(engine: &'a Engine) -> Self {
        Self {
            engine,
            fetched: RefCell::new(HashMap::new()),
        }
    }
}

impl PostingSource for QueryPostings<'_> {
    type Error = StoreError;

    fn postings(&self, term: &str) -> Result<Option<Arc<PostingList>>, StoreError> {
        if let Some(found) = self.fetched.borrow().get(term) {
            return Ok(found.clone());
        }
        let loaded = self.engine.load_postings(term)?;
        self.fetched.borrow_mut().insert(term.to_string(), loaded.clone());
        Ok(loaded)
    }
}
