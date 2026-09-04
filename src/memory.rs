//! Typed, explicit durable memory.
//!
//! Memory is an Orc-owned record, not model state.  User and experience
//! records use the authoritative global registry connection; project and
//! episodic records use the canonical project database.  This module contains
//! no inference, retrieval, ranking, or prompt integration.

use crate::storage::{Database, DbError};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MEMORY_SUBJECT_MAX_BYTES: usize = 256;
pub const MEMORY_CONTENT_MAX_BYTES: usize = 16 * 1024;
pub const MEMORY_PROVENANCE_REFERENCE_MAX_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    User,
    Project,
    Episodic,
    Experience,
}

impl MemoryKind {
    pub const fn is_global(self) -> bool {
        matches!(self, Self::User | Self::Experience)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
            Self::Episodic => "episodic",
            Self::Experience => "experience",
        }
    }

    pub fn parse(value: &str) -> Result<Self, MemoryError> {
        match value {
            "user" => Ok(Self::User),
            "project" => Ok(Self::Project),
            "episodic" => Ok(Self::Episodic),
            "experience" => Ok(Self::Experience),
            _ => Err(MemoryError::Invalid(format!(
                "unknown memory kind '{value}'"
            ))),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryScope {
    Global,
    Project { project_id: i64 },
}

impl MemoryScope {
    pub const fn project_id(&self) -> Option<i64> {
        match self {
            Self::Global => None,
            Self::Project { project_id } => Some(*project_id),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryId {
    Global(i64),
    Project { project_id: i64, id: i64 },
}

impl MemoryId {
    pub const fn scope(&self) -> MemoryScope {
        match self {
            Self::Global(_) => MemoryScope::Global,
            Self::Project { project_id, .. } => MemoryScope::Project {
                project_id: *project_id,
            },
        }
    }

    pub const fn value(&self) -> i64 {
        match self {
            Self::Global(id) => *id,
            Self::Project { id, .. } => *id,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryProvenanceKind {
    Operator,
    ProjectFact,
    ControllerApproved,
    Imported,
}

impl MemoryProvenanceKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Operator => "operator",
            Self::ProjectFact => "project_fact",
            Self::ControllerApproved => "controller_approved",
            Self::Imported => "imported",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self, MemoryError> {
        match value {
            "operator" => Ok(Self::Operator),
            "project_fact" => Ok(Self::ProjectFact),
            "controller_approved" => Ok(Self::ControllerApproved),
            "imported" => Ok(Self::Imported),
            _ => Err(MemoryError::Invalid(format!(
                "unknown memory provenance '{value}'"
            ))),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryProvenance {
    pub kind: MemoryProvenanceKind,
    pub source_reference: Option<String>,
}

impl MemoryProvenance {
    pub fn validate(&self) -> Result<(), MemoryError> {
        if let Some(reference) = &self.source_reference {
            validate_bounded(
                reference,
                "memory provenance source_reference",
                MEMORY_PROVENANCE_REFERENCE_MAX_BYTES,
            )?;
            if reference.trim().is_empty() {
                return Err(MemoryError::Invalid(
                    "memory provenance source_reference must not be empty".into(),
                ));
            }
        }
        if matches!(self.kind, MemoryProvenanceKind::Imported) && self.source_reference.is_none() {
            return Err(MemoryError::Invalid(
                "imported memory requires a source_reference".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryLifecycle {
    Active,
    Superseded,
    Removed,
}

impl MemoryLifecycle {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Superseded => "superseded",
            Self::Removed => "removed",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self, MemoryError> {
        match value {
            "active" => Ok(Self::Active),
            "superseded" => Ok(Self::Superseded),
            "removed" => Ok(Self::Removed),
            _ => Err(MemoryError::Invalid(format!(
                "unknown memory lifecycle '{value}'"
            ))),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MemoryDraft {
    pub kind: MemoryKind,
    pub scope: MemoryScope,
    pub subject: String,
    pub content: String,
    pub provenance: MemoryProvenance,
    pub confidence: Option<f64>,
}

impl MemoryDraft {
    pub fn validate(&self) -> Result<(), MemoryError> {
        validate_scope_kind(self.kind, &self.scope)?;
        validate_bounded(&self.subject, "memory subject", MEMORY_SUBJECT_MAX_BYTES)?;
        if self.subject.trim().is_empty() {
            return Err(MemoryError::Invalid(
                "memory subject must not be empty".into(),
            ));
        }
        validate_bounded(&self.content, "memory content", MEMORY_CONTENT_MAX_BYTES)?;
        if self.content.trim().is_empty() {
            return Err(MemoryError::Invalid(
                "memory content must not be empty".into(),
            ));
        }
        if let Some(project_id) = self.scope.project_id()
            && project_id <= 0
        {
            return Err(MemoryError::Invalid(
                "project memory requires a positive project_id".into(),
            ));
        }
        if let Some(confidence) = self.confidence
            && (!confidence.is_finite() || !(0.0..=1.0).contains(&confidence))
        {
            return Err(MemoryError::Invalid(
                "memory confidence must be finite and between 0 and 1".into(),
            ));
        }
        self.provenance.validate()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MemoryRecord {
    pub id: MemoryId,
    pub kind: MemoryKind,
    pub scope: MemoryScope,
    pub subject: String,
    pub content: String,
    pub provenance: MemoryProvenance,
    pub confidence: Option<f64>,
    pub lifecycle: MemoryLifecycle,
    pub supersedes: Option<MemoryId>,
    pub created_at: String,
    pub updated_at: String,
}

impl MemoryRecord {
    pub fn validate(&self) -> Result<(), MemoryError> {
        let draft = MemoryDraft {
            kind: self.kind,
            scope: self.scope.clone(),
            subject: self.subject.clone(),
            content: self.content.clone(),
            provenance: self.provenance.clone(),
            confidence: self.confidence,
        };
        draft.validate()?;
        if self.id.value() <= 0 {
            return Err(MemoryError::Invalid("memory ID must be positive".into()));
        }
        if self.id.scope() != self.scope {
            return Err(MemoryError::Invalid(
                "memory ID scope does not match record scope".into(),
            ));
        }
        if let Some(parent) = &self.supersedes {
            if parent.scope() != self.scope || parent.value() <= 0 {
                return Err(MemoryError::Invalid(
                    "memory supersession linkage has the wrong scope".into(),
                ));
            }
            if parent == &self.id {
                return Err(MemoryError::Invalid(
                    "memory cannot supersede itself".into(),
                ));
            }
        }
        if self.created_at.trim().is_empty() || self.updated_at.trim().is_empty() {
            return Err(MemoryError::Invalid(
                "memory timestamps must not be empty".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryQuery {
    pub scope: MemoryScope,
    pub kind: Option<MemoryKind>,
    pub subject: Option<String>,
    pub include_historical: bool,
}

impl MemoryQuery {
    pub fn active(scope: MemoryScope) -> Self {
        Self {
            scope,
            kind: None,
            subject: None,
            include_historical: false,
        }
    }

    pub fn validate(&self) -> Result<(), MemoryError> {
        if let Some(kind) = self.kind {
            validate_scope_kind(kind, &self.scope)?;
        }
        if let Some(subject) = &self.subject {
            validate_bounded(subject, "memory query subject", MEMORY_SUBJECT_MAX_BYTES)?;
            if subject.trim().is_empty() {
                return Err(MemoryError::Invalid(
                    "memory query subject must not be empty".into(),
                ));
            }
        }
        if let Some(project_id) = self.scope.project_id()
            && project_id <= 0
        {
            return Err(MemoryError::Invalid(
                "memory query requires a positive project_id".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum MemoryError {
    #[error("invalid memory: {0}")]
    Invalid(String),
}

fn validate_scope_kind(kind: MemoryKind, scope: &MemoryScope) -> Result<(), MemoryError> {
    if kind.is_global() != matches!(scope, MemoryScope::Global) {
        return Err(MemoryError::Invalid(format!(
            "memory kind '{}' cannot use scope {:?}",
            kind.as_str(),
            scope
        )));
    }
    Ok(())
}

fn validate_bounded(value: &str, field: &str, max_bytes: usize) -> Result<(), MemoryError> {
    if value.len() > max_bytes {
        return Err(MemoryError::Invalid(format!(
            "{field} is {} bytes; maximum is {max_bytes}",
            value.len()
        )));
    }
    Ok(())
}

/// Application-owned memory facade bound to the current Orc project.
pub struct MemoryService<'a> {
    db: &'a Database,
    project_id: i64,
}

impl<'a> MemoryService<'a> {
    pub(crate) const fn new(db: &'a Database, project_id: i64) -> Self {
        Self { db, project_id }
    }

    pub fn create(&self, draft: &MemoryDraft) -> Result<MemoryRecord, DbError> {
        if let MemoryScope::Project { project_id } = draft.scope
            && project_id != self.project_id
        {
            return Err(DbError::Scheduler(
                "project memory is outside the current application project".into(),
            ));
        }
        self.db.create_memory(draft)
    }

    pub fn get(&self, id: &MemoryId) -> Result<Option<MemoryRecord>, DbError> {
        if !self.owns_project_id(id) {
            return Ok(None);
        }
        self.db.get_memory(id)
    }

    pub fn list(
        &self,
        kind: Option<MemoryKind>,
        include_historical: bool,
    ) -> Result<Vec<MemoryRecord>, DbError> {
        let scope = if kind.is_some_and(MemoryKind::is_global) {
            MemoryScope::Global
        } else {
            MemoryScope::Project {
                project_id: self.project_id,
            }
        };
        self.db.list_memories(&MemoryQuery {
            scope,
            kind,
            subject: None,
            include_historical,
        })
    }

    pub fn correct(
        &self,
        id: &MemoryId,
        replacement: &MemoryDraft,
    ) -> Result<MemoryRecord, DbError> {
        self.require_owned_project_id(id)?;
        self.db.correct_memory(id, replacement)
    }

    pub fn supersede(
        &self,
        id: &MemoryId,
        replacement: &MemoryDraft,
    ) -> Result<MemoryRecord, DbError> {
        self.require_owned_project_id(id)?;
        self.db.supersede_memory(id, replacement)
    }

    pub fn remove(&self, id: &MemoryId) -> Result<MemoryRecord, DbError> {
        self.require_owned_project_id(id)?;
        self.db.remove_memory(id)
    }

    pub fn history(&self, id: &MemoryId) -> Result<Vec<MemoryRecord>, DbError> {
        self.require_owned_project_id(id)?;
        self.db.memory_history(id)
    }

    fn owns_project_id(&self, id: &MemoryId) -> bool {
        match id {
            MemoryId::Global(_) => true,
            MemoryId::Project { project_id, .. } => *project_id == self.project_id,
        }
    }

    fn require_owned_project_id(&self, id: &MemoryId) -> Result<(), DbError> {
        if self.owns_project_id(id) {
            Ok(())
        } else {
            Err(DbError::Scheduler(
                "memory belongs to another application project".into(),
            ))
        }
    }
}
