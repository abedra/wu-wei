use chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProjectId(pub Uuid);

impl ProjectId {
    pub fn new() -> Self {
        ProjectId(Uuid::new_v4())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectStatus {
    Active,
    OnHold,
    Completed,
    Dropped,
}

impl ProjectStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::OnHold => "on_hold",
            Self::Completed => "completed",
            Self::Dropped => "dropped",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "active" => Self::Active,
            "on_hold" => Self::OnHold,
            "completed" => Self::Completed,
            "dropped" => Self::Dropped,
            _ => return None,
        })
    }

    pub fn all() -> [Self; 4] {
        [Self::Active, Self::OnHold, Self::Completed, Self::Dropped]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Active => "Active",
            Self::OnHold => "On Hold",
            Self::Completed => "Completed",
            Self::Dropped => "Dropped",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectKind {
    Sequential,
    Parallel,
    SingleActions,
}

impl ProjectKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sequential => "sequential",
            Self::Parallel => "parallel",
            Self::SingleActions => "single_actions",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "sequential" => Self::Sequential,
            "parallel" => Self::Parallel,
            "single_actions" => Self::SingleActions,
            _ => return None,
        })
    }

    pub fn all() -> [Self; 3] {
        [Self::Sequential, Self::Parallel, Self::SingleActions]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Sequential => "Sequential",
            Self::Parallel => "Parallel",
            Self::SingleActions => "Single Actions",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub notes: String,
    pub status: ProjectStatus,
    pub kind: ProjectKind,
    pub created_at: DateTime<Utc>,
    /// Stamped by `project_repo::create`/`update` on every write, ignoring
    /// whatever's set here — callers never manage it by hand. Used by
    /// `sync` to pick the newer side of a conflicting edit.
    pub updated_at: DateTime<Utc>,
}

impl Project {
    pub fn new(name: impl Into<String>) -> Self {
        let now = Utc::now();
        Project {
            id: ProjectId::new(),
            name: name.into(),
            notes: String::new(),
            status: ProjectStatus::Active,
            kind: ProjectKind::Parallel,
            created_at: now,
            updated_at: now,
        }
    }
}
