use super::local::{
    gguf_metadata_cache_path, huggingface_hub_cache_dir, huggingface_identity_for_path,
};
use anyhow::{Context, Result};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct ModelUsageRecord {
    pub lookup_key: String,
    pub display_name: String,
    pub model_ref: Option<String>,
    pub source: String,
    pub mesh_managed: bool,
    pub primary_path: PathBuf,
    pub managed_paths: Vec<PathBuf>,
    pub first_seen_at: String,
    pub last_used_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ModelCleanupCandidate {
    pub display_name: String,
    pub model_ref: Option<String>,
    pub source: String,
    pub primary_path: PathBuf,
    pub mesh_managed: bool,
    pub last_used_at: String,
    pub file_count: usize,
    pub total_bytes: u64,
    pub stale_record_only: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ModelCleanupPlan {
    pub candidates: Vec<ModelCleanupCandidate>,
    pub total_files: usize,
    pub total_bytes: u64,
    pub skipped_recent: usize,
    pub stale_record_only: usize,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ModelCleanupResult {
    pub removed_candidates: usize,
    pub removed_files: usize,
    pub removed_records: usize,
    pub removed_metadata_files: usize,
    pub reclaimed_bytes: u64,
}

#[derive(Clone, Debug)]
struct CleanupEntry {
    record: ModelUsageRecord,
    record_path: PathBuf,
    removable_paths: Vec<PathBuf>,
    total_bytes: u64,
    stale_record_only: bool,
}

pub fn model_usage_cache_dir() -> PathBuf {
    super::mesh_llm_cache_dir().join("model-usage")
}

pub fn load_model_usage_record_for_path(path: &Path) -> Option<ModelUsageRecord> {
    let usage_dir = model_usage_cache_dir();
    let root = huggingface_hub_cache_dir();
    let lookup_key = usage_lookup_key(path, &root)?;
    let record_path = usage_record_path(&usage_dir, &lookup_key);
    read_usage_record(&record_path)
}

pub fn track_model_usage(
    path: &Path,
    display_name: Option<&str>,
    model_ref: Option<&str>,
    source: Option<&str>,
) -> Result<()> {
    let usage_dir = model_usage_cache_dir();
    let root = huggingface_hub_cache_dir();
    record_model_usage_in_dir(
        &usage_dir,
        &root,
        path,
        &[],
        display_name,
        model_ref,
        source,
        false,
    )
}

pub fn track_managed_model_usage(
    primary_path: &Path,
    managed_paths: &[PathBuf],
    display_name: &str,
    model_ref: Option<&str>,
    source: &str,
) -> Result<()> {
    let usage_dir = model_usage_cache_dir();
    let root = huggingface_hub_cache_dir();
    record_model_usage_in_dir(
        &usage_dir,
        &root,
        primary_path,
        managed_paths,
        Some(display_name),
        model_ref,
        Some(source),
        true,
    )
}

pub fn plan_model_cleanup(unused_since: Option<Duration>) -> Result<ModelCleanupPlan> {
    let usage_dir = model_usage_cache_dir();
    let root = huggingface_hub_cache_dir();
    plan_model_cleanup_in_dir(&usage_dir, &root, unused_since)
}

pub fn execute_model_cleanup(unused_since: Option<Duration>) -> Result<ModelCleanupResult> {
    let usage_dir = model_usage_cache_dir();
    let root = huggingface_hub_cache_dir();
    let records = load_model_usage_records_from_dir(&usage_dir);
    let cutoff = unused_since
        .map(ChronoDuration::from_std)
        .transpose()?
        .map(|age| Utc::now() - age);
    let mut skipped_recent = 0usize;
    let entries = plan_cleanup_entries(records, &usage_dir, &root, cutoff, &mut skipped_recent);
    execute_model_cleanup_entries(entries)
}

fn load_model_usage_records_from_dir(dir: &Path) -> Vec<ModelUsageRecord> {
    let mut records = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return records;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        if let Some(record) = read_usage_record(&path) {
            records.push(record);
        }
    }
    records
}

fn read_usage_record(path: &Path) -> Option<ModelUsageRecord> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn record_model_usage_in_dir(
    usage_dir: &Path,
    track_root: &Path,
    path: &Path,
    managed_paths: &[PathBuf],
    display_name: Option<&str>,
    model_ref: Option<&str>,
    source: Option<&str>,
    mesh_managed: bool,
) -> Result<()> {
    let Some(lookup_key) = usage_lookup_key(path, track_root) else {
        return Ok(());
    };

    let now = Utc::now().to_rfc3339();
    let record_path = usage_record_path(usage_dir, &lookup_key);
    let existing = read_usage_record(&record_path);
    let primary_path = normalize_path(path);
    let existing_display_name = existing
        .as_ref()
        .map(|record| record.display_name.as_str())
        .filter(|value| !value.is_empty());
    let existing_source = existing
        .as_ref()
        .map(|record| record.source.as_str())
        .filter(|value| !value.is_empty());
    let existing_model_ref = existing
        .as_ref()
        .and_then(|record| record.model_ref.as_deref());

    let mut merged_paths = existing
        .as_ref()
        .map(|record| record.managed_paths.clone())
        .unwrap_or_default();
    if mesh_managed {
        if managed_paths.is_empty() {
            merged_paths.push(primary_path.clone());
        } else {
            merged_paths.extend(managed_paths.iter().map(|path| normalize_path(path)));
        }
    }
    merged_paths = unique_paths(merged_paths);

    let record = ModelUsageRecord {
        lookup_key: lookup_key.clone(),
        display_name: display_name
            .or(existing_display_name)
            .map(str::to_string)
            .unwrap_or_else(|| default_display_name(&primary_path)),
        model_ref: model_ref
            .or(existing_model_ref)
            .map(str::to_string)
            .or_else(|| default_model_ref(&primary_path)),
        source: source
            .or(existing_source)
            .map(str::to_string)
            .unwrap_or_else(|| default_source(&primary_path)),
        mesh_managed: mesh_managed || existing.as_ref().is_some_and(|record| record.mesh_managed),
        primary_path,
        managed_paths: merged_paths,
        first_seen_at: existing
            .as_ref()
            .map(|record| record.first_seen_at.clone())
            .unwrap_or_else(|| now.clone()),
        last_used_at: now,
    };

    std::fs::create_dir_all(usage_dir)
        .with_context(|| format!("Create {}", usage_dir.display()))?;
    let bytes = serde_json::to_vec_pretty(&record)?;
    std::fs::write(&record_path, bytes)
        .with_context(|| format!("Write {}", record_path.display()))?;
    Ok(())
}

fn plan_model_cleanup_in_dir(
    usage_dir: &Path,
    track_root: &Path,
    unused_since: Option<Duration>,
) -> Result<ModelCleanupPlan> {
    let records = load_model_usage_records_from_dir(usage_dir);
    let cutoff = unused_since
        .map(ChronoDuration::from_std)
        .transpose()?
        .map(|age| Utc::now() - age);
    let mut skipped_recent = 0usize;
    let entries = plan_cleanup_entries(records, usage_dir, track_root, cutoff, &mut skipped_recent);
    let mut plan = ModelCleanupPlan::default();
    plan.skipped_recent = skipped_recent;
    for entry in entries {
        if entry.stale_record_only {
            plan.stale_record_only += 1;
        }
        plan.total_files += entry.removable_paths.len();
        plan.total_bytes += entry.total_bytes;
        plan.candidates.push(ModelCleanupCandidate {
            display_name: entry.record.display_name,
            model_ref: entry.record.model_ref,
            source: entry.record.source,
            primary_path: entry.record.primary_path,
            mesh_managed: entry.record.mesh_managed,
            last_used_at: entry.record.last_used_at,
            file_count: entry.removable_paths.len(),
            total_bytes: entry.total_bytes,
            stale_record_only: entry.stale_record_only,
        });
    }
    plan.candidates.sort_by(|left, right| {
        left.last_used_at
            .cmp(&right.last_used_at)
            .then_with(|| left.display_name.cmp(&right.display_name))
    });
    Ok(plan)
}

fn plan_cleanup_entries(
    records: Vec<ModelUsageRecord>,
    usage_dir: &Path,
    track_root: &Path,
    cutoff: Option<DateTime<Utc>>,
    skipped_recent: &mut usize,
) -> Vec<CleanupEntry> {
    let mut entries = Vec::new();

    for record in records {
        if !record.mesh_managed {
            continue;
        }
        let last_used =
            parse_timestamp(&record.last_used_at).unwrap_or(DateTime::<Utc>::UNIX_EPOCH);
        if let Some(cutoff) = cutoff {
            if last_used > cutoff {
                *skipped_recent += 1;
                continue;
            }
        }

        let removable_paths: Vec<PathBuf> = unique_paths(record.managed_paths.clone())
            .into_iter()
            .filter(|path| is_trackable_path(path, track_root))
            .filter(|path| path.exists())
            .collect();
        let total_bytes = removable_paths
            .iter()
            .filter_map(|path| std::fs::metadata(path).ok().map(|meta| meta.len()))
            .sum();
        let stale_record_only = removable_paths.is_empty();
        let record_path = usage_record_path(usage_dir, &record.lookup_key);

        entries.push(CleanupEntry {
            record,
            record_path,
            removable_paths,
            total_bytes,
            stale_record_only,
        });
    }

    entries.sort_by(|left, right| {
        left.record
            .last_used_at
            .cmp(&right.record.last_used_at)
            .then_with(|| left.record.display_name.cmp(&right.record.display_name))
    });
    entries
}

fn execute_model_cleanup_entries(entries: Vec<CleanupEntry>) -> Result<ModelCleanupResult> {
    let mut result = ModelCleanupResult::default();
    for entry in entries {
        for path in &entry.removable_paths {
            if let Ok(meta) = std::fs::metadata(path) {
                result.reclaimed_bytes += meta.len();
            }
            if path.exists() {
                std::fs::remove_file(path).with_context(|| format!("Remove {}", path.display()))?;
                result.removed_files += 1;
            }
            if let Some(cache_path) = gguf_metadata_cache_path(path) {
                if cache_path.exists() {
                    std::fs::remove_file(&cache_path).with_context(|| {
                        format!("Remove metadata cache {}", cache_path.display())
                    })?;
                    result.removed_metadata_files += 1;
                }
            }
            prune_empty_ancestors(path, &huggingface_hub_cache_dir());
        }
        if entry.record_path.exists() {
            std::fs::remove_file(&entry.record_path)
                .with_context(|| format!("Remove {}", entry.record_path.display()))?;
            result.removed_records += 1;
        }
        result.removed_candidates += 1;
    }
    Ok(result)
}

fn usage_lookup_key(path: &Path, track_root: &Path) -> Option<String> {
    if !is_trackable_path(path, track_root) {
        return None;
    }
    if let Some(identity) = huggingface_identity_for_path(path) {
        return Some(format!("hf:{}", identity.canonical_ref));
    }
    Some(format!(
        "path:{}",
        normalize_path(path).to_string_lossy().replace('\\', "/")
    ))
}

fn usage_record_path(usage_dir: &Path, lookup_key: &str) -> PathBuf {
    let digest = Sha256::digest(lookup_key.as_bytes());
    usage_dir.join(format!("{digest:x}.json"))
}

fn normalize_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn unique_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut unique = Vec::new();
    for path in paths {
        let normalized = normalize_path(&path);
        if seen.insert(normalized.clone()) {
            unique.push(normalized);
        }
    }
    unique.sort();
    unique
}

fn parse_timestamp(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|parsed| parsed.with_timezone(&Utc))
}

fn default_display_name(path: &Path) -> String {
    path.file_stem()
        .and_then(|value| value.to_str())
        .or_else(|| path.file_name().and_then(|value| value.to_str()))
        .unwrap_or("model")
        .to_string()
}

fn default_model_ref(path: &Path) -> Option<String> {
    huggingface_identity_for_path(path).map(|identity| identity.canonical_ref)
}

fn default_source(path: &Path) -> String {
    if huggingface_identity_for_path(path).is_some() {
        "huggingface-cache".to_string()
    } else {
        "local-cache".to_string()
    }
}

fn is_trackable_path(path: &Path, track_root: &Path) -> bool {
    let path = normalize_path(path);
    let root = normalize_path(track_root);
    path.starts_with(&root)
}

fn prune_empty_ancestors(path: &Path, stop_at: &Path) {
    let stop_at = normalize_path(stop_at);
    let mut current = path.parent().map(normalize_path);
    while let Some(dir) = current {
        if dir == stop_at {
            break;
        }
        let Ok(mut entries) = std::fs::read_dir(&dir) else {
            break;
        };
        if entries.next().is_some() {
            break;
        }
        if std::fs::remove_dir(&dir).is_err() {
            break;
        }
        current = dir.parent().map(normalize_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(prefix: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "{prefix}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time should be after epoch")
                .as_nanos()
        ))
    }

    fn write_record(dir: &Path, record: &ModelUsageRecord) {
        std::fs::create_dir_all(dir).expect("usage dir should be created");
        let path = usage_record_path(dir, &record.lookup_key);
        std::fs::write(
            path,
            serde_json::to_vec_pretty(record).expect("record JSON should serialize"),
        )
        .expect("record should be written");
    }

    #[test]
    fn record_model_usage_merges_managed_paths() {
        let usage_dir = temp_dir("mesh-llm-usage-dir");
        let cache_root = temp_dir("mesh-llm-hf-cache");
        let primary = cache_root
            .join("models--Org--Demo")
            .join("snapshots")
            .join("rev1")
            .join("Demo-Q4_K_M.gguf");
        let shard = cache_root
            .join("models--Org--Demo")
            .join("snapshots")
            .join("rev1")
            .join("Demo-Q4_K_M-00002-of-00002.gguf");
        std::fs::create_dir_all(primary.parent().expect("primary path should have parent"))
            .expect("primary parent should exist");
        std::fs::write(&primary, b"primary").expect("primary model should be written");
        std::fs::write(&shard, b"shard").expect("shard model should be written");

        record_model_usage_in_dir(
            &usage_dir,
            &cache_root,
            &primary,
            &[primary.clone(), shard.clone()],
            Some("Demo-Q4_K_M"),
            Some("Org/Demo@rev1/Demo-Q4_K_M.gguf"),
            Some("catalog"),
            true,
        )
        .expect("managed usage should be recorded");

        let records = load_model_usage_records_from_dir(&usage_dir);
        assert_eq!(records.len(), 1);
        assert!(records[0].mesh_managed);
        assert_eq!(records[0].managed_paths.len(), 2);
        assert_eq!(records[0].display_name, "Demo-Q4_K_M");

        let _ = std::fs::remove_dir_all(&usage_dir);
        let _ = std::fs::remove_dir_all(&cache_root);
    }

    #[test]
    fn cleanup_plan_filters_recent_and_external_records() {
        let usage_dir = temp_dir("mesh-llm-usage-dir");
        let cache_root = temp_dir("mesh-llm-hf-cache");
        let old_path = cache_root
            .join("models--Org--Old")
            .join("snapshots")
            .join("rev1")
            .join("Old-Q4_K_M.gguf");
        let recent_path = cache_root
            .join("models--Org--Recent")
            .join("snapshots")
            .join("rev1")
            .join("Recent-Q4_K_M.gguf");
        let external_path = cache_root
            .join("models--Org--External")
            .join("snapshots")
            .join("rev1")
            .join("External-Q4_K_M.gguf");

        for path in [&old_path, &recent_path, &external_path] {
            std::fs::create_dir_all(path.parent().expect("test path should have parent"))
                .expect("test parent should exist");
            std::fs::write(path, vec![0_u8; 16]).expect("test model should be written");
        }

        write_record(
            &usage_dir,
            &ModelUsageRecord {
                lookup_key: usage_lookup_key(&old_path, &cache_root).expect("old path should key"),
                display_name: "Old".to_string(),
                model_ref: None,
                source: "catalog".to_string(),
                mesh_managed: true,
                primary_path: old_path.clone(),
                managed_paths: vec![old_path.clone()],
                first_seen_at: "2026-04-01T00:00:00Z".to_string(),
                last_used_at: "2026-04-01T00:00:00Z".to_string(),
            },
        );
        write_record(
            &usage_dir,
            &ModelUsageRecord {
                lookup_key: usage_lookup_key(&recent_path, &cache_root)
                    .expect("recent path should key"),
                display_name: "Recent".to_string(),
                model_ref: None,
                source: "catalog".to_string(),
                mesh_managed: true,
                primary_path: recent_path.clone(),
                managed_paths: vec![recent_path.clone()],
                first_seen_at: Utc::now().to_rfc3339(),
                last_used_at: Utc::now().to_rfc3339(),
            },
        );
        write_record(
            &usage_dir,
            &ModelUsageRecord {
                lookup_key: usage_lookup_key(&external_path, &cache_root)
                    .expect("external path should key"),
                display_name: "External".to_string(),
                model_ref: None,
                source: "local-cache".to_string(),
                mesh_managed: false,
                primary_path: external_path.clone(),
                managed_paths: vec![],
                first_seen_at: "2026-04-01T00:00:00Z".to_string(),
                last_used_at: "2026-04-01T00:00:00Z".to_string(),
            },
        );

        let plan =
            plan_model_cleanup_in_dir(&usage_dir, &cache_root, Some(Duration::from_secs(60)))
                .expect("cleanup plan should succeed");
        assert_eq!(plan.candidates.len(), 1);
        assert_eq!(plan.candidates[0].display_name, "Old");
        assert_eq!(plan.skipped_recent, 1);

        let _ = std::fs::remove_dir_all(&usage_dir);
        let _ = std::fs::remove_dir_all(&cache_root);
    }

    #[test]
    fn execute_cleanup_removes_files_and_records() {
        let usage_dir = temp_dir("mesh-llm-usage-dir");
        let cache_root = temp_dir("mesh-llm-hf-cache");
        let primary = cache_root
            .join("models--Org--Cleanup")
            .join("snapshots")
            .join("rev1")
            .join("Cleanup-Q4_K_M.gguf");
        std::fs::create_dir_all(primary.parent().expect("cleanup path should have parent"))
            .expect("cleanup parent should exist");
        std::fs::write(&primary, vec![0_u8; 32]).expect("cleanup model should be written");

        let record = ModelUsageRecord {
            lookup_key: usage_lookup_key(&primary, &cache_root).expect("cleanup path should key"),
            display_name: "Cleanup".to_string(),
            model_ref: Some("Org/Cleanup@rev1/Cleanup-Q4_K_M.gguf".to_string()),
            source: "catalog".to_string(),
            mesh_managed: true,
            primary_path: primary.clone(),
            managed_paths: vec![primary.clone()],
            first_seen_at: "2026-04-01T00:00:00Z".to_string(),
            last_used_at: "2026-04-01T00:00:00Z".to_string(),
        };
        write_record(&usage_dir, &record);

        let mut skipped_recent = 0usize;
        let entries = plan_cleanup_entries(
            vec![record],
            &usage_dir,
            &cache_root,
            None,
            &mut skipped_recent,
        );
        let result = execute_model_cleanup_entries(entries).expect("cleanup should succeed");
        assert_eq!(result.removed_candidates, 1);
        assert_eq!(result.removed_files, 1);
        assert_eq!(result.removed_records, 1);
        assert!(!primary.exists());
        assert!(load_model_usage_records_from_dir(&usage_dir).is_empty());

        let _ = std::fs::remove_dir_all(&usage_dir);
        let _ = std::fs::remove_dir_all(&cache_root);
    }
}
