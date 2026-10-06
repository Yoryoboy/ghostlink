//! Community insight contracts, independent of serialization and transport.
use crate::{AppError, Timestamp};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClarityInsight {
    pub hash: u32,
    pub name: String,
    pub kind: String,
    pub item_hash: Option<u32>,
    pub item_name: Option<String>,
    pub text: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClarityMetadata {
    pub version: String,
    pub checked_at: Timestamp,
    pub updated_at: Timestamp,
    pub entries: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClarityBatch {
    pub entries: Vec<ClarityInsight>,
    pub skipped: usize,
}

pub trait ClarityGateway {
    fn version(&mut self) -> Result<String, AppError>;
    fn descriptions(&mut self) -> Result<ClarityBatch, AppError>;
}

pub trait ClarityRepository {
    fn clarity_by_hash(&self, hash: u32) -> Result<Option<ClarityInsight>, AppError>;
    fn clarity_by_name(&self, name: &str) -> Result<Vec<ClarityInsight>, AppError>;
    fn clarity_metadata(&self) -> Result<Option<ClarityMetadata>, AppError>;
    /// Replaces the entire set and metadata atomically.
    fn publish_clarity(
        &mut self,
        version: &str,
        entries: &[ClarityInsight],
        checked_at: Timestamp,
    ) -> Result<(), AppError>;
    fn mark_clarity_checked(&mut self, checked_at: Timestamp) -> Result<(), AppError>;
}
