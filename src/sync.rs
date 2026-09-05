//! Multi-device sync over a shared folder (Dropbox, iCloud Drive, a network
//! share, a USB stick — anything that behaves like a folder). Each device
//! keeps its own private local database; a sync run writes this device's
//! full current state to `<folder>/<device-id>.json`, reads every other
//! device's file there, and merges the winning state into the local db.
//! There is no live shared connection — see the conversation this design
//! came out of for why that's unsafe over a network filesystem.
//!
//! Conflict rule: a deletion is final. If any source (local or any remote)
//! has a tombstone for an id, that id is deleted — full stop — even if
//! another source's edit to it has a later timestamp. Otherwise, the
//! version with the latest `updated_at` wins. Ids are UUIDs generated
//! client-side, so two devices creating new items concurrently never
//! collide, and a first sync between two devices with independent
//! histories is just a union.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::thread;

use chrono::{DateTime, NaiveDate, Utc};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db::error::DbResult;
use crate::db::{project_repo, sync_repo, task_repo};
use crate::domain::project::{Project, ProjectId, ProjectKind, ProjectStatus};
use crate::domain::task::{Recurrence, RecurrenceUnit, Task, TaskId, WeekdaySet};

/// Flat, JSON-serializable mirror of `domain::task::Task` — sync's wire
/// format doesn't derive serde on the domain type directly, matching how
/// `llm/prompt.rs` keeps its own JSON-boundary structs separate from
/// domain ones. `PartialEq` backs `already_current`'s whole-record
/// comparison — see its doc comment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncTask {
    pub id: String,
    pub title: String,
    pub notes: String,
    pub project_id: Option<String>,
    pub due_date: Option<NaiveDate>,
    pub defer_date: Option<NaiveDate>,
    pub completed: bool,
    pub completed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub estimated_minutes: Option<i64>,
    /// Lower is higher priority; `None` = unset. `#[serde(default)]` so
    /// payloads from an older peer that predates the field still
    /// deserialize — see `recurrence_weekday_mask`.
    #[serde(default)]
    pub priority: Option<i64>,
    pub recurrence_interval: Option<u32>,
    pub recurrence_unit: Option<String>,
    /// Mon-first weekday bitmask restricting which days a repeat lands on
    /// (see `domain::task::WeekdaySet`). `#[serde(default)]` so payloads
    /// written by an older peer that predates the field still deserialize.
    #[serde(default)]
    pub recurrence_weekday_mask: Option<u8>,
}

impl From<&Task> for SyncTask {
    fn from(t: &Task) -> Self {
        SyncTask {
            id: t.id.0.to_string(),
            title: t.title.clone(),
            notes: t.notes.clone(),
            project_id: t.project_id.map(|p| p.0.to_string()),
            due_date: t.due_date,
            defer_date: t.defer_date,
            completed: t.completed,
            completed_at: t.completed_at,
            created_at: t.created_at,
            updated_at: t.updated_at,
            estimated_minutes: t.estimated_minutes,
            priority: t.priority,
            recurrence_interval: t.recurrence.map(|r| r.interval),
            recurrence_unit: t.recurrence.map(|r| r.unit.as_str().to_string()),
            recurrence_weekday_mask: t.recurrence.and_then(|r| r.weekdays).map(|w| w.to_mask()),
        }
    }
}

impl SyncTask {
    /// Resolves `project_id` against `surviving_projects` — `None` if it
    /// isn't in that set, so a task whose project was deleted (or never
    /// arrived, having been created and deleted on another device before
    /// reaching this one) falls back to Inbox instead of failing to merge.
    fn into_task(self, surviving_projects: &HashSet<String>) -> Option<Task> {
        let id = Uuid::parse_str(&self.id).ok()?;
        let project_id = self
            .project_id
            .filter(|p| surviving_projects.contains(p))
            .and_then(|p| Uuid::parse_str(&p).ok())
            .map(ProjectId);
        let recurrence = self
            .recurrence_interval
            .zip(self.recurrence_unit.as_deref())
            .and_then(|(interval, unit)| {
                RecurrenceUnit::parse(unit).map(|unit| {
                    Recurrence::every(interval, unit)
                        .with_weekdays(self.recurrence_weekday_mask.map(WeekdaySet::from_mask))
                })
            });
        Some(Task {
            id: TaskId(id),
            title: self.title,
            notes: self.notes,
            project_id,
            due_date: self.due_date,
            defer_date: self.defer_date,
            completed: self.completed,
            completed_at: self.completed_at,
            created_at: self.created_at,
            updated_at: self.updated_at,
            estimated_minutes: self.estimated_minutes,
            priority: self.priority,
            recurrence,
        })
    }
}

/// Flat, JSON-serializable mirror of `domain::project::Project` — see
/// `SyncTask`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncProject {
    pub id: String,
    pub name: String,
    pub notes: String,
    pub status: String,
    pub kind: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<&Project> for SyncProject {
    fn from(p: &Project) -> Self {
        SyncProject {
            id: p.id.0.to_string(),
            name: p.name.clone(),
            notes: p.notes.clone(),
            status: p.status.as_str().to_string(),
            kind: p.kind.as_str().to_string(),
            created_at: p.created_at,
            updated_at: p.updated_at,
        }
    }
}

impl SyncProject {
    fn into_project(self) -> Option<Project> {
        let id = Uuid::parse_str(&self.id).ok()?;
        let status = ProjectStatus::parse(&self.status)?;
        let kind = ProjectKind::parse(&self.kind)?;
        Some(Project {
            id: ProjectId(id),
            name: self.name,
            notes: self.notes,
            status,
            kind,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncTombstone {
    pub kind: String,
    pub id: String,
    pub deleted_at: DateTime<Utc>,
}

impl From<&sync_repo::Tombstone> for SyncTombstone {
    fn from(t: &sync_repo::Tombstone) -> Self {
        SyncTombstone {
            kind: t.kind.clone(),
            id: t.id.clone(),
            deleted_at: t.deleted_at,
        }
    }
}

/// A full snapshot of one device's state — what gets written to
/// `<folder>/<device_id>.json` and read back from every other device's
/// file there. Not an incremental log: simplest possible design at this
/// app's scale, and trivially idempotent to re-merge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub device_id: String,
    pub synced_at: DateTime<Utc>,
    pub tasks: Vec<SyncTask>,
    pub projects: Vec<SyncProject>,
    pub tombstones: Vec<SyncTombstone>,
}

/// A sync run's result, for the status line the UI shows near the sync
/// button.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncSummary {
    pub tasks_upserted: usize,
    pub tasks_deleted: usize,
    pub projects_upserted: usize,
    pub projects_deleted: usize,
}

impl SyncSummary {
    pub fn describe(&self) -> String {
        if *self == SyncSummary::default() {
            return "Already up to date".to_string();
        }
        let mut parts = Vec::new();
        if self.tasks_upserted > 0 {
            parts.push(format!("{} task(s) updated", self.tasks_upserted));
        }
        if self.tasks_deleted > 0 {
            parts.push(format!("{} task(s) removed", self.tasks_deleted));
        }
        if self.projects_upserted > 0 {
            parts.push(format!("{} project(s) updated", self.projects_upserted));
        }
        if self.projects_deleted > 0 {
            parts.push(format!("{} project(s) removed", self.projects_deleted));
        }
        parts.join(", ")
    }
}

pub fn local_snapshot(conn: &Connection, device_id: &str) -> DbResult<Snapshot> {
    let tasks = task_repo::list_all(conn)?
        .iter()
        .map(SyncTask::from)
        .collect();
    let projects = project_repo::list_all(conn)?
        .iter()
        .map(SyncProject::from)
        .collect();
    let tombstones = sync_repo::list_tombstones(conn)?
        .iter()
        .map(SyncTombstone::from)
        .collect();
    Ok(Snapshot {
        device_id: device_id.to_string(),
        synced_at: Utc::now(),
        tasks,
        projects,
        tombstones,
    })
}

/// Writes atomically (temp file + rename) so another device reading the
/// folder mid-write sees either the old file or the new one, never a
/// half-written one.
pub fn write_snapshot(folder: &Path, snapshot: &Snapshot) -> std::io::Result<()> {
    fs::create_dir_all(folder)?;
    let final_path = folder.join(format!("{}.json", snapshot.device_id));
    let tmp_path = folder.join(format!("{}.json.tmp", snapshot.device_id));
    let json = serde_json::to_string_pretty(snapshot)
        .expect("Snapshot always serializes: plain data, no non-finite floats");
    fs::write(&tmp_path, json)?;
    fs::rename(&tmp_path, &final_path)?;
    Ok(())
}

/// Every other device's snapshot file in `folder` (skipping `device_id`'s
/// own). A file that fails to parse — e.g. caught mid-write by another
/// device despite the atomic rename above, or simply stale/foreign — is
/// skipped rather than failing the whole sync run.
pub fn read_remote_snapshots(folder: &Path, device_id: &str) -> std::io::Result<Vec<Snapshot>> {
    let mut snapshots = Vec::new();
    let entries = match fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(snapshots),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if path.file_stem().and_then(|s| s.to_str()) == Some(device_id) {
            continue;
        }
        let Ok(contents) = fs::read_to_string(&path) else {
            continue;
        };
        if let Ok(snapshot) = serde_json::from_str::<Snapshot>(&contents) {
            snapshots.push(snapshot);
        }
    }
    Ok(snapshots)
}

/// The merge itself: applies the winning state (per the module doc
/// comment's conflict rule) to `conn`. Projects are resolved and written
/// before tasks, since a task's `project_id` foreign key needs its project
/// to already exist locally (`PRAGMA foreign_keys = ON` is set in
/// `db::open`).
pub fn merge(conn: &Connection, local: &Snapshot, remotes: &[Snapshot]) -> DbResult<SyncSummary> {
    let mut summary = SyncSummary::default();

    let project_tombstones = tombstones_by_id(local, remotes, "project");
    let project_versions = winning_versions(
        local
            .projects
            .iter()
            .chain(remotes.iter().flat_map(|r| r.projects.iter())),
        |p| p.id.clone(),
        |p| p.updated_at,
        // Projects have no completion state, so this is purely the same
        // deterministic, order-independent fallback tasks use below.
        |cand, existing| deterministic_tiebreak(cand, existing),
        &project_tombstones,
    );
    let local_project_ids: HashSet<&str> = local.projects.iter().map(|p| p.id.as_str()).collect();

    for (id, deleted_at) in &project_tombstones {
        if let Ok(uuid) = Uuid::parse_str(id) {
            if local_project_ids.contains(id.as_str()) {
                summary.projects_deleted += 1;
            }
            project_repo::delete_if_exists(conn, ProjectId(uuid))?;
        }
        sync_repo::record_tombstone(conn, "project", id, *deleted_at)?;
    }
    for (id, version) in &project_versions {
        if already_current(&local.projects, id, version, |p| p.id.as_str()) {
            continue;
        }
        if let Some(project) = version.clone().into_project() {
            project_repo::upsert_synced(conn, &project)?;
            summary.projects_upserted += 1;
        }
    }
    let surviving_project_ids: HashSet<String> = project_versions.keys().cloned().collect();

    let task_tombstones = tombstones_by_id(local, remotes, "task");
    let task_versions = winning_versions(
        local
            .tasks
            .iter()
            .chain(remotes.iter().flat_map(|r| r.tasks.iter())),
        |t| t.id.clone(),
        |t| t.updated_at,
        // A completion beats a not-completed copy at an equal timestamp — see
        // `winning_versions`. Ordinarily only fires on legacy ties, since new
        // edits all carry a distinct `updated_at` — but if two versions ever
        // do tie *and* disagree on some other field too (e.g. a stray direct
        // DB edit that didn't bump `updated_at`), fall through to a
        // deterministic, content-based tiebreak so every device converges on
        // the same winner instead of each one permanently keeping its own
        // copy (see `winning_versions`'s doc comment).
        |cand, existing| {
            if cand.completed != existing.completed {
                cand.completed && !existing.completed
            } else {
                deterministic_tiebreak(cand, existing)
            }
        },
        &task_tombstones,
    );
    let local_task_ids: HashSet<&str> = local.tasks.iter().map(|t| t.id.as_str()).collect();

    for (id, deleted_at) in &task_tombstones {
        if let Ok(uuid) = Uuid::parse_str(id) {
            if local_task_ids.contains(id.as_str()) {
                summary.tasks_deleted += 1;
            }
            task_repo::delete_if_exists(conn, TaskId(uuid))?;
        }
        sync_repo::record_tombstone(conn, "task", id, *deleted_at)?;
    }
    for (id, version) in &task_versions {
        if already_current(&local.tasks, id, version, |t| t.id.as_str()) {
            continue;
        }
        if let Some(task) = version.clone().into_task(&surviving_project_ids) {
            task_repo::upsert_synced(conn, &task)?;
            summary.tasks_upserted += 1;
        }
    }

    Ok(summary)
}

/// The latest `deleted_at` recorded for each id of the given `kind`, across
/// local and every remote's tombstones.
fn tombstones_by_id(
    local: &Snapshot,
    remotes: &[Snapshot],
    kind: &str,
) -> HashMap<String, DateTime<Utc>> {
    let mut result: HashMap<String, DateTime<Utc>> = HashMap::new();
    for t in local
        .tombstones
        .iter()
        .chain(remotes.iter().flat_map(|r| r.tombstones.iter()))
        .filter(|t| t.kind == kind)
    {
        result
            .entry(t.id.clone())
            .and_modify(|existing| *existing = (*existing).max(t.deleted_at))
            .or_insert(t.deleted_at);
    }
    result
}

/// The latest (by `updated_at`) version of each id across every source,
/// excluding any id that has a tombstone — a tombstone always wins,
/// regardless of any upsert's timestamp.
///
/// `prefer_on_tie(candidate, existing)` breaks an exact `updated_at` tie: when
/// it returns true the candidate replaces the existing pick. Without it, a
/// tie would resolve to whichever source happens to be iterated first —
/// `local` is always chained ahead of `remotes` at the call site below, so
/// naively that would mean every device permanently prefers its *own* copy
/// on a tie, and two devices could disagree forever, no matter how many more
/// times they sync. `prefer_on_tie` must therefore be symmetric — give the
/// same answer regardless of which side is `cand` versus `existing` — so
/// every device converges on the same winner; see `deterministic_tiebreak`,
/// the fallback both call sites below end on.
fn winning_versions<'a, T: Clone + 'a>(
    items: impl Iterator<Item = &'a T>,
    id_of: impl Fn(&T) -> String,
    updated_at_of: impl Fn(&T) -> DateTime<Utc>,
    prefer_on_tie: impl Fn(&T, &T) -> bool,
    tombstones: &HashMap<String, DateTime<Utc>>,
) -> HashMap<String, T> {
    let mut result: HashMap<String, T> = HashMap::new();
    for item in items {
        let id = id_of(item);
        if tombstones.contains_key(&id) {
            continue;
        }
        result
            .entry(id)
            .and_modify(|existing| {
                let item_at = updated_at_of(item);
                let existing_at = updated_at_of(existing);
                if item_at > existing_at
                    || (item_at == existing_at && prefer_on_tie(item, existing))
                {
                    *existing = item.clone();
                }
            })
            .or_insert_with(|| item.clone());
    }
    result
}

/// An order-independent tie-break for `winning_versions`: prefers whichever
/// of `cand`/`existing` serializes to the lexicographically greater JSON
/// string. Which one specifically wins is arbitrary — the only property that
/// matters is that it's the same answer everywhere, since `cand` and
/// `existing` swap roles depending on which side happens to be iterated
/// first (see `winning_versions`'s doc comment).
fn deterministic_tiebreak<T: Serialize>(cand: &T, existing: &T) -> bool {
    let cand_json = serde_json::to_string(cand).unwrap_or_default();
    let existing_json = serde_json::to_string(existing).unwrap_or_default();
    cand_json > existing_json
}

/// Whether `id`'s winning version is exactly what's already stored
/// locally — if so, `merge` skips writing it, both as a minor efficiency
/// and so the summary only counts what actually changed. Compares the
/// *whole* record rather than a hand-picked subset of fields: a fingerprint
/// of only some fields (e.g. just `updated_at`/`completed`) can miss a
/// winning version that differs only in some other field a tie-break can
/// flip (see `winning_versions`'s `prefer_on_tie`) — a version that looked
/// "already current" by the fingerprint would then be wrongly skipped and
/// never actually applied.
fn already_current<T: PartialEq>(
    local_items: &[T],
    id: &str,
    winning_version: &T,
    id_of: impl Fn(&T) -> &str,
) -> bool {
    local_items
        .iter()
        .any(|item| id_of(item) == id && item == winning_version)
}

/// Orchestrates one full sync round: writes this device's current state to
/// the shared folder, reads every other device's, and merges the winning
/// result into the local database. `device_id` should be a stable
/// per-installation identifier (see `AppState`'s `sync_device_id`).
pub fn run(conn: &Connection, folder: &Path, device_id: &str) -> Result<SyncSummary, String> {
    let local = local_snapshot(conn, device_id).map_err(|e| e.to_string())?;
    write_snapshot(folder, &local).map_err(|e| e.to_string())?;
    let remotes = read_remote_snapshots(folder, device_id).map_err(|e| e.to_string())?;
    merge(conn, &local, &remotes).map_err(|e| e.to_string())
}

/// Runs `run` on a background thread, opening its own connection to
/// `db_path` — the same local file the caller's own connection is already
/// open on. Safe: same-machine multi-connection SQLite access is exactly
/// what its locking is designed for (see `db::open`'s `busy_timeout`),
/// unlike sharing one database file *between machines* (what this whole
/// module exists to avoid). Delivered over the returned channel, polled
/// once per frame — mirrors `llm::parse_capture_async`.
pub fn run_async(
    db_path: String,
    folder: String,
    device_id: String,
) -> Receiver<Result<SyncSummary, String>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = crate::db::open(&db_path)
            .map_err(|e| e.to_string())
            .and_then(|conn| run(&conn, Path::new(&folder), &device_id));
        let _ = tx.send(result);
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn remote_with(tasks: Vec<SyncTask>, tombstones: Vec<SyncTombstone>) -> Snapshot {
        Snapshot {
            device_id: "device-b".to_string(),
            synced_at: Utc::now(),
            tasks,
            projects: Vec::new(),
            tombstones,
        }
    }

    #[test]
    fn sync_task_carries_the_recurrence_weekday_restriction_both_ways() {
        use crate::domain::task::{Recurrence, RecurrenceUnit, WeekdaySet};

        let mut task = Task::new_inbox("standup");
        task.recurrence = Some(
            Recurrence::every(1, RecurrenceUnit::Days).with_weekdays(Some(WeekdaySet::WEEKDAYS)),
        );

        let wire = SyncTask::from(&task);
        assert_eq!(
            wire.recurrence_weekday_mask,
            Some(WeekdaySet::WEEKDAYS.to_mask())
        );

        let back = wire.into_task(&HashSet::new()).unwrap();
        assert_eq!(back.recurrence, task.recurrence);
    }

    #[test]
    fn sync_task_carries_priority_both_ways() {
        let mut task = Task::new_inbox("triage");
        task.priority = Some(2);

        let wire = SyncTask::from(&task);
        assert_eq!(wire.priority, Some(2));

        let back = wire.into_task(&HashSet::new()).unwrap();
        assert_eq!(back.priority, Some(2));
    }

    #[test]
    fn sync_task_from_a_peer_predating_the_priority_field_still_deserializes() {
        // No `priority` key — `#[serde(default)]` fills None.
        let json = r#"{
            "id": "3d2e1f00-0000-4000-8000-000000000000",
            "title": "old task", "notes": "", "project_id": null,
            "due_date": null, "defer_date": null, "completed": false,
            "completed_at": null,
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
            "estimated_minutes": null,
            "recurrence_interval": null, "recurrence_unit": null
        }"#;
        let wire: SyncTask = serde_json::from_str(json).unwrap();
        assert_eq!(wire.priority, None);
        let task = wire.into_task(&HashSet::new()).unwrap();
        assert_eq!(task.priority, None);
    }

    #[test]
    fn sync_task_from_a_peer_predating_the_weekday_field_still_deserializes() {
        // No `recurrence_weekday_mask` key — `#[serde(default)]` fills None.
        let json = r#"{
            "id": "3d2e1f00-0000-4000-8000-000000000000",
            "title": "old task", "notes": "", "project_id": null,
            "due_date": null, "defer_date": null, "completed": false,
            "completed_at": null,
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
            "estimated_minutes": null,
            "recurrence_interval": 1, "recurrence_unit": "weeks"
        }"#;
        let wire: SyncTask = serde_json::from_str(json).unwrap();
        assert_eq!(wire.recurrence_weekday_mask, None);
        let task = wire.into_task(&HashSet::new()).unwrap();
        assert_eq!(
            task.recurrence,
            Some(crate::domain::task::Recurrence::every(
                1,
                crate::domain::task::RecurrenceUnit::Weeks
            ))
        );
    }

    #[test]
    fn disjoint_new_tasks_from_two_devices_merge_to_their_union() {
        let conn = db::open_in_memory().unwrap();
        task_repo::create(&conn, &Task::new_inbox("local task")).unwrap();
        let local = local_snapshot(&conn, "device-a").unwrap();

        let remote = remote_with(
            vec![SyncTask::from(&Task::new_inbox("remote task"))],
            vec![],
        );

        let summary = merge(&conn, &local, std::slice::from_ref(&remote)).unwrap();

        assert_eq!(summary.tasks_upserted, 1);
        let all = task_repo::list_all(&conn).unwrap();
        assert_eq!(all.len(), 2);
        assert!(all.iter().any(|t| t.title == "local task"));
        assert!(all.iter().any(|t| t.title == "remote task"));
    }

    #[test]
    fn conflicting_edits_resolve_to_the_later_updated_at() {
        let conn = db::open_in_memory().unwrap();
        let task = Task::new_inbox("shared task");
        task_repo::create(&conn, &task).unwrap();
        let local = local_snapshot(&conn, "device-a").unwrap();

        let mut remote_task = local.tasks[0].clone();
        remote_task.title = "edited remotely, later".to_string();
        remote_task.updated_at = local.tasks[0].updated_at + chrono::Duration::seconds(10);
        let remote = remote_with(vec![remote_task], vec![]);

        merge(&conn, &local, std::slice::from_ref(&remote)).unwrap();

        let fetched = task_repo::get(&conn, task.id).unwrap().unwrap();
        assert_eq!(fetched.title, "edited remotely, later");
    }

    #[test]
    fn a_tied_timestamp_with_differing_fields_converges_regardless_of_merge_direction() {
        // A genuine `updated_at` collision between two devices shouldn't
        // happen in normal use (every real edit gets a fresh `Utc::now()`
        // stamp), but if two versions of the same task ever do tie while
        // disagreeing on some other field (e.g. a direct DB edit that
        // skipped the normal update path), every device must land on the
        // *same* winner — not each permanently keep its own copy just
        // because it happened to be "local" in its own merge.
        let base = Task::new_inbox("shared task");
        let mut version_a = base.clone();
        version_a.priority = Some(1);
        let mut version_b = base.clone();
        version_b.priority = Some(4);
        assert_eq!(version_a.updated_at, version_b.updated_at, "test setup: must tie");

        // Merge with `version_a` actually stored locally, `version_b` arriving
        // as a remote snapshot... `upsert_synced` (not `create`) so the row's
        // `updated_at` is exactly `version_a`'s, not a freshly stamped one —
        // `local_snapshot` must see the same tie the remote does.
        let conn_a = db::open_in_memory().unwrap();
        task_repo::upsert_synced(&conn_a, &version_a).unwrap();
        let local_a = local_snapshot(&conn_a, "device-a").unwrap();
        let remote_b = remote_with(vec![SyncTask::from(&version_b)], vec![]);
        merge(&conn_a, &local_a, std::slice::from_ref(&remote_b)).unwrap();
        let resolved_a = task_repo::get(&conn_a, base.id).unwrap().unwrap();

        // ...and the other way around: `version_b` stored locally, `version_a`
        // arriving as remote.
        let conn_b = db::open_in_memory().unwrap();
        task_repo::upsert_synced(&conn_b, &version_b).unwrap();
        let local_b = local_snapshot(&conn_b, "device-b").unwrap();
        let remote_a = remote_with(vec![SyncTask::from(&version_a)], vec![]);
        merge(&conn_b, &local_b, std::slice::from_ref(&remote_a)).unwrap();
        let resolved_b = task_repo::get(&conn_b, base.id).unwrap().unwrap();

        assert_eq!(
            resolved_a.priority, resolved_b.priority,
            "the two devices converged on different priorities"
        );
    }

    #[test]
    fn a_completion_wins_a_timestamp_tie_against_a_stale_open_copy() {
        // The completed-tasks-resurrected bug: before completion stamped
        // `updated_at`, one device could complete a task while another kept it
        // open, both at the same `updated_at`. The merge must land on the
        // completed version regardless of source order, in both directions.
        let conn = db::open_in_memory().unwrap();
        let local = local_snapshot(&conn, "device-a").unwrap();

        let task = Task::new_inbox("Fix arrow key/tab navigation");
        let mut open_copy = SyncTask::from(&task);
        open_copy.completed = false;
        let mut done_copy = SyncTask::from(&task);
        done_copy.completed = true;
        done_copy.completed_at = Some(task.updated_at); // same instant, no bump
        assert_eq!(open_copy.updated_at, done_copy.updated_at);

        for (first, second) in [
            (open_copy.clone(), done_copy.clone()),
            (done_copy.clone(), open_copy.clone()),
        ] {
            let conn = db::open_in_memory().unwrap();
            let remotes = [
                remote_with(vec![first], vec![]),
                remote_with(vec![second], vec![]),
            ];
            merge(&conn, &local, &remotes).unwrap();

            let fetched = task_repo::get(&conn, task.id).unwrap().unwrap();
            assert!(fetched.completed, "completion lost the tie");
            assert!(task_repo::list_inbox(&conn).unwrap().is_empty());
            assert_eq!(task_repo::list_completed(&conn).unwrap().len(), 1);
        }
    }

    #[test]
    fn a_completion_heals_a_device_already_holding_the_stale_open_copy() {
        // Even a device that already stored the open copy at the tied timestamp
        // must adopt the completion — `already_current` has to notice they
        // differ despite the equal `updated_at`.
        let conn = db::open_in_memory().unwrap();
        let mut task = Task::new_inbox("Add file selector to sync settings");
        task_repo::create(&conn, &task).unwrap();
        // Force local's updated_at to a known instant we can tie against.
        task = task_repo::get(&conn, task.id).unwrap().unwrap();
        let local = local_snapshot(&conn, "device-a").unwrap();
        assert!(!local.tasks[0].completed);

        let mut done_copy = local.tasks[0].clone();
        done_copy.completed = true;
        done_copy.completed_at = Some(task.updated_at); // equal updated_at
        let remote = remote_with(vec![done_copy], vec![]);

        let summary = merge(&conn, &local, std::slice::from_ref(&remote)).unwrap();
        assert_eq!(
            summary.tasks_upserted, 1,
            "heal was skipped as already-current"
        );
        assert!(task_repo::get(&conn, task.id).unwrap().unwrap().completed);
    }

    #[test]
    fn a_delete_always_wins_over_a_later_edit() {
        let conn = db::open_in_memory().unwrap();
        let task = Task::new_inbox("about to be deleted");
        task_repo::create(&conn, &task).unwrap();
        task_repo::delete(&conn, task.id).unwrap();
        let local = local_snapshot(&conn, "device-a").unwrap();
        assert!(local.tasks.is_empty());
        assert_eq!(local.tombstones.len(), 1);

        // A remote edit timestamped *after* the delete.
        let mut remote_task = SyncTask::from(&task);
        remote_task.updated_at = local.tombstones[0].deleted_at + chrono::Duration::seconds(10);
        let remote = remote_with(vec![remote_task], vec![]);

        merge(&conn, &local, std::slice::from_ref(&remote)).unwrap();

        assert!(task_repo::get(&conn, task.id).unwrap().is_none());
    }

    #[test]
    fn a_task_whose_project_was_deleted_elsewhere_falls_back_to_inbox() {
        let conn = db::open_in_memory().unwrap();
        let local = local_snapshot(&conn, "device-a").unwrap();

        let project = Project::new("Remote-only project");
        let mut task = Task::new_inbox("references a project we've never seen");
        task.project_id = Some(project.id);

        let remote = Snapshot {
            device_id: "device-b".to_string(),
            synced_at: Utc::now(),
            tasks: vec![SyncTask::from(&task)],
            // The project itself never arrives...
            projects: vec![],
            // ...because it was deleted before this device heard of it.
            tombstones: vec![SyncTombstone {
                kind: "project".to_string(),
                id: project.id.0.to_string(),
                deleted_at: Utc::now(),
            }],
        };

        merge(&conn, &local, std::slice::from_ref(&remote)).unwrap();

        let fetched = task_repo::get(&conn, task.id).unwrap().unwrap();
        assert!(fetched.project_id.is_none());
    }

    #[test]
    fn merging_the_same_remote_snapshot_twice_is_idempotent() {
        let conn = db::open_in_memory().unwrap();
        let local = local_snapshot(&conn, "device-a").unwrap();
        let remote = remote_with(
            vec![SyncTask::from(&Task::new_inbox("from device b"))],
            vec![],
        );

        let first = merge(&conn, &local, std::slice::from_ref(&remote)).unwrap();
        assert_eq!(first.tasks_upserted, 1);

        let local_after = local_snapshot(&conn, "device-a").unwrap();
        let second = merge(&conn, &local_after, std::slice::from_ref(&remote)).unwrap();

        assert_eq!(second, SyncSummary::default());
        assert_eq!(task_repo::list_all(&conn).unwrap().len(), 1);
    }

    #[test]
    fn write_and_read_remote_snapshots_round_trips_through_a_temp_dir() {
        let dir = std::env::temp_dir().join(format!("wu-wei-sync-test-{}", Uuid::new_v4()));
        let snapshot = Snapshot {
            device_id: "device-a".to_string(),
            synced_at: Utc::now(),
            tasks: vec![SyncTask::from(&Task::new_inbox("round trip"))],
            projects: vec![],
            tombstones: vec![],
        };

        write_snapshot(&dir, &snapshot).unwrap();

        let remotes = read_remote_snapshots(&dir, "device-b").unwrap();
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].tasks[0].title, "round trip");

        let none_for_self = read_remote_snapshots(&dir, "device-a").unwrap();
        assert!(none_for_self.is_empty());

        fs::remove_dir_all(&dir).ok();
    }
}
