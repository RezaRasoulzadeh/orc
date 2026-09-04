//! Bounded, read-only Controller memory context.
//!
//! This module projects active records from the canonical M06-001
//! [`crate::memory::MemoryService`]. It deliberately does not own persistence,
//! inference, prompts, or any Controller capability request/state type.

use crate::memory::{
    MemoryId, MemoryKind, MemoryLifecycle, MemoryProvenance, MemoryScope, MemoryService,
};
use crate::storage::DbError;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const CONTROLLER_MEMORY_CONTEXT_VERSION: u32 = 1;
pub const MAX_CONTROLLER_MEMORY_ITEMS_PER_KIND: usize = 8;
pub const MAX_CONTROLLER_MEMORY_ITEMS: usize = 24;
pub const MAX_CONTROLLER_MEMORY_CONTEXT_BYTES: usize = 32 * 1024;
pub const MAX_CONTROLLER_MEMORY_SUBJECT_BYTES: usize = 256;
pub const MAX_CONTROLLER_MEMORY_CONTENT_BYTES: usize = 4096;
pub const MAX_CONTROLLER_MEMORY_SOURCE_REFERENCE_BYTES: usize = 512;

/// Authority/category retained in the projection so later capability-specific
/// requests can distinguish current project facts from durable history.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerMemoryAuthority {
    CurrentProject,
    DurableUser,
    ProjectHistory,
    CrossProjectExperience,
}

impl ControllerMemoryAuthority {
    fn for_record(kind: MemoryKind, scope: &MemoryScope) -> Option<Self> {
        match (kind, scope) {
            (MemoryKind::Project, MemoryScope::Project { .. }) => Some(Self::CurrentProject),
            (MemoryKind::User, MemoryScope::Global) => Some(Self::DurableUser),
            (MemoryKind::Episodic, MemoryScope::Project { .. }) => Some(Self::ProjectHistory),
            (MemoryKind::Experience, MemoryScope::Global) => Some(Self::CrossProjectExperience),
            _ => None,
        }
    }
}

/// One active canonical memory projected for future Controller capability
/// inputs. The scope, provenance, lifecycle, and supersession link remain
/// typed rather than being flattened into undifferentiated text.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerMemoryItem {
    pub id: MemoryId,
    pub kind: MemoryKind,
    pub scope: MemoryScope,
    pub authority: ControllerMemoryAuthority,
    pub subject: String,
    pub content: String,
    pub provenance: MemoryProvenance,
    pub confidence: Option<f64>,
    pub lifecycle: MemoryLifecycle,
    pub supersedes: Option<MemoryId>,
}

impl ControllerMemoryItem {
    fn from_record(record: &crate::memory::MemoryRecord) -> Option<Self> {
        if record.validate().is_err()
            || record.lifecycle != MemoryLifecycle::Active
            || record.id.scope() != record.scope
            || record
                .supersedes
                .as_ref()
                .is_some_and(|parent| parent.scope() != record.scope || parent == &record.id)
            || record.subject.len() > MAX_CONTROLLER_MEMORY_SUBJECT_BYTES
            || record.content.len() > MAX_CONTROLLER_MEMORY_CONTENT_BYTES
            || record
                .provenance
                .source_reference
                .as_ref()
                .is_some_and(|source| source.len() > MAX_CONTROLLER_MEMORY_SOURCE_REFERENCE_BYTES)
        {
            return None;
        }
        let authority = ControllerMemoryAuthority::for_record(record.kind, &record.scope)?;
        Some(Self {
            id: record.id.clone(),
            kind: record.kind,
            scope: record.scope.clone(),
            authority,
            subject: record.subject.clone(),
            content: record.content.clone(),
            provenance: record.provenance.clone(),
            confidence: record.confidence,
            lifecycle: record.lifecycle,
            supersedes: record.supersedes.clone(),
        })
    }

    pub fn validate(&self) -> Result<(), ControllerMemoryError> {
        if self.lifecycle != MemoryLifecycle::Active {
            return Err(ControllerMemoryError::InvalidItem(
                "Controller memory context may contain active records only".into(),
            ));
        }
        if self.id.scope() != self.scope {
            return Err(ControllerMemoryError::InvalidItem(
                "memory item ID scope does not match its scope".into(),
            ));
        }
        if self.id.value() <= 0
            || self
                .scope
                .project_id()
                .is_some_and(|project_id| project_id <= 0)
        {
            return Err(ControllerMemoryError::InvalidItem(
                "memory item identity must be positive".into(),
            ));
        }
        if ControllerMemoryAuthority::for_record(self.kind, &self.scope) != Some(self.authority) {
            return Err(ControllerMemoryError::InvalidItem(
                "memory item authority does not match kind and scope".into(),
            ));
        }
        validate_text(
            &self.subject,
            MAX_CONTROLLER_MEMORY_SUBJECT_BYTES,
            "subject",
        )?;
        validate_text(
            &self.content,
            MAX_CONTROLLER_MEMORY_CONTENT_BYTES,
            "content",
        )?;
        self.provenance
            .validate()
            .map_err(|error| ControllerMemoryError::InvalidItem(error.to_string()))?;
        if let Some(source) = &self.provenance.source_reference {
            validate_text(
                source,
                MAX_CONTROLLER_MEMORY_SOURCE_REFERENCE_BYTES,
                "provenance.source_reference",
            )?;
        }
        if let Some(confidence) = self.confidence
            && (!confidence.is_finite() || !(0.0..=1.0).contains(&confidence))
        {
            return Err(ControllerMemoryError::InvalidItem(
                "memory item confidence must be finite and between 0 and 1".into(),
            ));
        }
        if let Some(parent) = &self.supersedes
            && (parent.scope() != self.scope || parent == &self.id)
        {
            return Err(ControllerMemoryError::InvalidItem(
                "memory item supersession linkage has the wrong scope".into(),
            ));
        }
        Ok(())
    }
}

/// Reusable bounded Controller memory projection. It is intentionally
/// independent of `ControllerStatePacket`, planning, review, intake, and
/// recovery request/state types so each capability can embed it later.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerMemoryContext {
    pub context_version: u32,
    pub items: Vec<ControllerMemoryItem>,
}

#[derive(Debug, Error)]
pub enum ControllerMemoryError {
    #[error("controller memory read failed: {0}")]
    Storage(#[source] DbError),
    #[error("controller memory context serialization failed: {0}")]
    Serialization(String),
    #[error("controller memory context is {actual} bytes; maximum is {max}")]
    ContextTooLarge { actual: usize, max: usize },
    #[error("controller memory context is invalid: {0}")]
    InvalidItem(String),
    #[error("controller memory context exceeds its {field} bound")]
    Bounds { field: String },
}

impl ControllerMemoryContext {
    /// Construct an empty context without consulting persistence.
    pub const fn empty() -> Self {
        Self {
            context_version: CONTROLLER_MEMORY_CONTEXT_VERSION,
            items: Vec::new(),
        }
    }

    /// Read active records through the canonical M06-001 memory service.
    ///
    /// Records over the projection's per-item bounds are omitted
    /// deterministically; content is never silently cut at a UTF-8 boundary.
    /// The fixed kind order follows the architecture's durable-memory
    /// distinction: current project facts, user preferences, project history,
    /// then cross-project experience.
    pub fn from_memory_service(service: &MemoryService<'_>) -> Result<Self, ControllerMemoryError> {
        let ordered_kinds = [
            MemoryKind::Project,
            MemoryKind::User,
            MemoryKind::Episodic,
            MemoryKind::Experience,
        ];
        let mut candidates = Vec::new();
        for kind in ordered_kinds {
            let records = service
                .list(Some(kind), false)
                .map_err(ControllerMemoryError::Storage)?;
            candidates.extend(
                records
                    .into_iter()
                    .filter_map(|record| {
                        ControllerMemoryItem::from_record(&record).map(|item| {
                            (kind_order(kind), record.created_at, record.id.value(), item)
                        })
                    })
                    .take(MAX_CONTROLLER_MEMORY_ITEMS_PER_KIND),
            );
        }
        candidates.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.cmp(&right.1))
                .then_with(|| left.2.cmp(&right.2))
        });

        let mut items = Vec::new();
        for (_, _, _, item) in candidates.into_iter().take(MAX_CONTROLLER_MEMORY_ITEMS) {
            let mut candidate = Self {
                context_version: CONTROLLER_MEMORY_CONTEXT_VERSION,
                items: items.clone(),
            };
            candidate.items.push(item);
            if serialized_size(&candidate)? <= MAX_CONTROLLER_MEMORY_CONTEXT_BYTES {
                items = candidate.items;
            }
        }
        let context = Self {
            context_version: CONTROLLER_MEMORY_CONTEXT_VERSION,
            items,
        };
        context.validate()?;
        Ok(context)
    }

    pub fn validate(&self) -> Result<(), ControllerMemoryError> {
        if self.context_version != CONTROLLER_MEMORY_CONTEXT_VERSION {
            return Err(ControllerMemoryError::InvalidItem(
                "unsupported Controller memory context version".into(),
            ));
        }
        if self.items.len() > MAX_CONTROLLER_MEMORY_ITEMS {
            return Err(ControllerMemoryError::Bounds {
                field: "total items".into(),
            });
        }
        let mut ids = Vec::new();
        let mut per_kind = [0usize; 4];
        for item in &self.items {
            item.validate()?;
            if ids.contains(&item.id) {
                return Err(ControllerMemoryError::InvalidItem(
                    "duplicate memory identity".into(),
                ));
            }
            ids.push(item.id.clone());
            per_kind[kind_order(item.kind)] += 1;
        }
        if per_kind
            .iter()
            .any(|count| *count > MAX_CONTROLLER_MEMORY_ITEMS_PER_KIND)
        {
            return Err(ControllerMemoryError::Bounds {
                field: "items per memory kind".into(),
            });
        }
        let actual = serialized_size(self)?;
        if actual > MAX_CONTROLLER_MEMORY_CONTEXT_BYTES {
            return Err(ControllerMemoryError::ContextTooLarge {
                actual,
                max: MAX_CONTROLLER_MEMORY_CONTEXT_BYTES,
            });
        }
        Ok(())
    }
}

fn kind_order(kind: MemoryKind) -> usize {
    match kind {
        MemoryKind::Project => 0,
        MemoryKind::User => 1,
        MemoryKind::Episodic => 2,
        MemoryKind::Experience => 3,
    }
}

fn serialized_size(context: &ControllerMemoryContext) -> Result<usize, ControllerMemoryError> {
    serde_json::to_vec(context)
        .map(|bytes| bytes.len())
        .map_err(|error| ControllerMemoryError::Serialization(error.to_string()))
}

fn validate_text(value: &str, max_bytes: usize, field: &str) -> Result<(), ControllerMemoryError> {
    if value.trim().is_empty() || value.len() > max_bytes {
        return Err(ControllerMemoryError::Bounds {
            field: field.into(),
        });
    }
    Ok(())
}
