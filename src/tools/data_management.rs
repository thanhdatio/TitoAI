//! Data management tool for ZeroClaw: retention, purge, export, GDPR erasure, and stats.
//!
//! Provides data lifecycle management operations including retention status,
//! time-based purging (with dry-run), data export, GDPR-compliant erasure,
//! and workspace storage statistics.

use super::traits::{Tool, ToolResult};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::json;
use std::path::{Path, PathBuf};
use tokio::fs;

/// DataManagementTool exposes data retention and lifecycle management to the agent.
pub struct DataManagementTool {
    workspace_dir: PathBuf,
    retention_days: u64,
    gdpr_erasure_enabled: bool,
    erasure_stores: Vec<String>,
}

impl DataManagementTool {
    pub fn new(
        workspace_dir: PathBuf,
        retention_days: u64,
        gdpr_erasure_enabled: bool,
        erasure_stores: Vec<String>,
    ) -> Self {
        Self {
            workspace_dir,
            retention_days,
            gdpr_erasure_enabled,
            erasure_stores,
        }
    }

    async fn retention_status(&self) -> anyhow::Result<ToolResult> {
        let mut report = Vec::new();
        report.push(format!("Retention policy: {} days", self.retention_days));
        report.push(format!(
            "GDPR erasure: {}",
            if self.gdpr_erasure_enabled {
                "enabled"
            } else {
                "disabled"
            }
        ));
        report.push(format!(
            "Erasure stores: {}",
            self.erasure_stores.join(", ")
        ));

        let cutoff = Utc::now() - chrono::Duration::days(self.retention_days as i64);
        report.push(format!(
            "Data older than {} is eligible for purge.",
            cutoff.format("%Y-%m-%d")
        ));

        Ok(ToolResult {
            success: true,
            output: report.join("\n"),
            error: None,
        })
    }

    async fn purge_data(
        &self,
        older_than_days: Option<u64>,
        dry_run: bool,
    ) -> anyhow::Result<ToolResult> {
        let days = older_than_days.unwrap_or(self.retention_days);
        let cutoff = Utc::now() - chrono::Duration::days(days as i64);
        let cutoff_ts = cutoff.timestamp();

        let mut total_eligible = 0usize;
        let mut total_purged = 0usize;
        let mut details = Vec::new();

        for store in &self.erasure_stores {
            let store_dir = self.workspace_dir.join(store);
            if !store_dir.exists() {
                continue;
            }
            let (eligible, purged) = purge_old_files(&store_dir, cutoff_ts, dry_run).await?;
            total_eligible += eligible;
            total_purged += purged;
            if eligible > 0 {
                let suffix = if dry_run {
                    " (dry-run, not deleted)".to_string()
                } else {
                    format!(", {purged} purged")
                };
                details.push(format!("  {store}: {eligible} eligible{suffix}"));
            }
        }

        let mode = if dry_run { "DRY-RUN" } else { "PURGE" };
        let summary = if details.is_empty() {
            format!("[{mode}] No files older than {days} days found.")
        } else {
            format!(
                "[{mode}] Files older than {days} days: {total_eligible} eligible, {total_purged} purged.\n{}",
                details.join("\n")
            )
        };

        Ok(ToolResult {
            success: true,
            output: summary,
            error: None,
        })
    }

    async fn export_data(&self, format: &str) -> anyhow::Result<ToolResult> {
        match format {
            "json" => {
                let mut data = serde_json::Map::new();
                for store in &self.erasure_stores {
                    let store_dir = self.workspace_dir.join(store);
                    if !store_dir.exists() {
                        continue;
                    }
                    let files = collect_file_list(&store_dir, store).await?;
                    data.insert(
                        store.clone(),
                        serde_json::Value::Array(
                            files
                                .into_iter()
                                .map(serde_json::Value::String)
                                .collect(),
                        ),
                    );
                }
                let export_json =
                    serde_json::to_string_pretty(&serde_json::Value::Object(data))?;
                Ok(ToolResult {
                    success: true,
                    output: format!("Export (JSON):\n{export_json}"),
                    error: None,
                })
            }
            "csv" => {
                let mut lines = vec!["store,file".to_string()];
                for store in &self.erasure_stores {
                    let store_dir = self.workspace_dir.join(store);
                    if !store_dir.exists() {
                        continue;
                    }
                    let files = collect_file_list(&store_dir, store).await?;
                    for f in files {
                        lines.push(format!("{store},{f}"));
                    }
                }
                Ok(ToolResult {
                    success: true,
                    output: format!("Export (CSV):\n{}", lines.join("\n")),
                    error: None,
                })
            }
            other => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "Unsupported export format: '{other}'. Supported: json, csv."
                )),
            }),
        }
    }

    async fn gdpr_erasure(&self, user_id: &str) -> anyhow::Result<ToolResult> {
        if !self.gdpr_erasure_enabled {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(
                    "GDPR erasure is not enabled. Set data_retention.gdpr_erasure_enabled = true."
                        .to_string(),
                ),
            });
        }

        let mut total_erased = 0usize;
        let mut details = Vec::new();

        for store in &self.erasure_stores {
            let store_dir = self.workspace_dir.join(store);
            if !store_dir.exists() {
                continue;
            }
            let erased = erase_user_data(&store_dir, user_id).await?;
            if erased > 0 {
                details.push(format!("  {store}: {erased} files erased"));
                total_erased += erased;
            }
        }

        let summary = if total_erased == 0 {
            format!("GDPR erasure: no data found for user '{user_id}'.")
        } else {
            format!(
                "GDPR erasure complete for user '{user_id}': {total_erased} files erased.\n{}",
                details.join("\n")
            )
        };

        Ok(ToolResult {
            success: true,
            output: summary,
            error: None,
        })
    }

    async fn storage_stats(&self) -> anyhow::Result<ToolResult> {
        let mut lines = Vec::new();
        let mut total_size = 0u64;
        let mut total_files = 0usize;

        for store in &self.erasure_stores {
            let store_dir = self.workspace_dir.join(store);
            if !store_dir.exists() {
                lines.push(format!("  {store}: (not present)"));
                continue;
            }
            let (count, size) = dir_stats(&store_dir).await?;
            total_files += count;
            total_size += size;
            lines.push(format!(
                "  {store}: {count} files, {}",
                human_bytes(size)
            ));
        }

        lines.insert(
            0,
            format!(
                "Workspace storage: {total_files} files, {}",
                human_bytes(total_size)
            ),
        );

        Ok(ToolResult {
            success: true,
            output: lines.join("\n"),
            error: None,
        })
    }
}

#[async_trait]
impl Tool for DataManagementTool {
    fn name(&self) -> &str {
        "data_management"
    }

    fn description(&self) -> &str {
        "Manage ZeroClaw data lifecycle: retention status, purge old data (with dry-run), export, GDPR user erasure, and storage stats."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["retention_status", "purge", "export", "erasure", "stats"],
                    "description": "Data management action"
                },
                "older_than_days": {
                    "type": "integer",
                    "description": "Purge files older than N days (default: retention_days from config)"
                },
                "dry_run": {
                    "type": "boolean",
                    "description": "If true, show what would be purged without deleting"
                },
                "format": {
                    "type": "string",
                    "enum": ["json", "csv"],
                    "description": "Export format"
                },
                "user_id": {
                    "type": "string",
                    "description": "User identifier for GDPR erasure"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'action' parameter"))?;

        match action {
            "retention_status" => self.retention_status().await,
            "purge" => {
                let older_than = args.get("older_than_days").and_then(|v| v.as_u64());
                let dry_run = args
                    .get("dry_run")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                self.purge_data(older_than, dry_run).await
            }
            "export" => {
                let format = args
                    .get("format")
                    .and_then(|v| v.as_str())
                    .unwrap_or("json");
                self.export_data(format).await
            }
            "erasure" => {
                let user_id = args
                    .get("user_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing 'user_id' for erasure"))?;
                self.gdpr_erasure(user_id).await
            }
            "stats" => self.storage_stats().await,
            other => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Unknown data_management action: '{other}'")),
            }),
        }
    }
}

/// Recursively find and count files older than the given cutoff timestamp.
/// When `dry_run` is false, delete them.
async fn purge_old_files(
    dir: &Path,
    cutoff_ts: i64,
    dry_run: bool,
) -> anyhow::Result<(usize, usize)> {
    let mut eligible = 0usize;
    let mut purged = 0usize;

    let mut read_dir = fs::read_dir(dir).await?;
    while let Some(entry) = read_dir.next_entry().await? {
        let path = entry.path();
        let file_type = entry.file_type().await?;
        if file_type.is_dir() {
            let (e, p) = Box::pin(purge_old_files(&path, cutoff_ts, dry_run)).await?;
            eligible += e;
            purged += p;
        } else if file_type.is_file() {
            if let Ok(meta) = entry.metadata().await {
                if let Ok(modified) = meta.modified() {
                    let mod_ts = modified
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i64;
                    if mod_ts < cutoff_ts {
                        eligible += 1;
                        if !dry_run {
                            if fs::remove_file(&path).await.is_ok() {
                                purged += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    Ok((eligible, purged))
}

/// Recursively collect file paths under a directory.
async fn collect_file_list(dir: &Path, prefix: &str) -> anyhow::Result<Vec<String>> {
    let mut files = Vec::new();
    let mut read_dir = fs::read_dir(dir).await?;
    while let Some(entry) = read_dir.next_entry().await? {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let rel = format!("{prefix}/{name}");
        let file_type = entry.file_type().await?;
        if file_type.is_dir() {
            let sub = Box::pin(collect_file_list(&path, &rel)).await?;
            files.extend(sub);
        } else if file_type.is_file() {
            files.push(rel);
        }
    }
    Ok(files)
}

/// Search all files in a store directory for content mentioning `user_id` and delete matches.
async fn erase_user_data(store_dir: &Path, user_id: &str) -> anyhow::Result<usize> {
    let mut erased = 0usize;
    let mut read_dir = fs::read_dir(store_dir).await?;
    while let Some(entry) = read_dir.next_entry().await? {
        let path = entry.path();
        let file_type = entry.file_type().await?;
        if file_type.is_dir() {
            let sub = Box::pin(erase_user_data(&path, user_id)).await?;
            erased += sub;
        } else if file_type.is_file() {
            let name = entry.file_name().to_string_lossy().to_string();
            let mut should_erase = name.contains(user_id);
            if !should_erase {
                if let Ok(content) = fs::read_to_string(&path).await {
                    should_erase = content.contains(user_id);
                }
            }
            if should_erase {
                if fs::remove_file(&path).await.is_ok() {
                    erased += 1;
                }
            }
        }
    }
    Ok(erased)
}

/// Recursively compute total file count and byte size.
async fn dir_stats(dir: &Path) -> anyhow::Result<(usize, u64)> {
    let mut count = 0usize;
    let mut size = 0u64;
    let mut read_dir = fs::read_dir(dir).await?;
    while let Some(entry) = read_dir.next_entry().await? {
        let path = entry.path();
        let file_type = entry.file_type().await?;
        if file_type.is_dir() {
            let (c, s) = Box::pin(dir_stats(&path)).await?;
            count += c;
            size += s;
        } else if file_type.is_file() {
            if let Ok(meta) = entry.metadata().await {
                count += 1;
                size += meta.len();
            }
        }
    }
    Ok((count, size))
}

fn human_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_tool(ws: &Path) -> DataManagementTool {
        DataManagementTool::new(
            ws.to_path_buf(),
            90,
            true,
            vec!["memory".to_string(), "audit".to_string()],
        )
    }

    #[tokio::test]
    async fn retention_status_shows_policy() {
        let tmp = TempDir::new().unwrap();
        let tool = make_tool(tmp.path());
        let result = tool.retention_status().await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("90 days"));
    }

    #[tokio::test]
    async fn purge_dry_run_does_not_delete() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();

        let memory_dir = ws.join("memory");
        fs::create_dir_all(&memory_dir).await.unwrap();
        let old_file = memory_dir.join("old.txt");
        fs::write(&old_file, b"old data").await.unwrap();

        // Set modification time to 200 days ago.
        let old_time =
            std::time::SystemTime::now() - std::time::Duration::from_secs(200 * 24 * 3600);
        filetime::set_file_mtime(
            &old_file,
            filetime::FileTime::from_system_time(old_time),
        )
        .unwrap();

        let tool = make_tool(ws);
        let result = tool.purge_data(Some(90), true).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("DRY-RUN"));
        assert!(result.output.contains("1 eligible"));
        assert!(old_file.exists());
    }

    #[tokio::test]
    async fn purge_actually_deletes_old_files() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();

        let audit_dir = ws.join("audit");
        fs::create_dir_all(&audit_dir).await.unwrap();
        let old_file = audit_dir.join("log.txt");
        fs::write(&old_file, b"audit entry").await.unwrap();

        let old_time =
            std::time::SystemTime::now() - std::time::Duration::from_secs(200 * 24 * 3600);
        filetime::set_file_mtime(
            &old_file,
            filetime::FileTime::from_system_time(old_time),
        )
        .unwrap();

        let tool = make_tool(ws);
        let result = tool.purge_data(Some(90), false).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("PURGE"));
        assert!(!old_file.exists());
    }

    #[tokio::test]
    async fn stats_reports_file_counts() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();

        let memory_dir = ws.join("memory");
        fs::create_dir_all(&memory_dir).await.unwrap();
        fs::write(memory_dir.join("a.txt"), b"aaa").await.unwrap();
        fs::write(memory_dir.join("b.txt"), b"bbb").await.unwrap();

        let tool = make_tool(ws);
        let result = tool.storage_stats().await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("2 files"));
    }

    #[tokio::test]
    async fn erasure_removes_user_files() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();

        let memory_dir = ws.join("memory");
        fs::create_dir_all(&memory_dir).await.unwrap();
        fs::write(memory_dir.join("user_abc.txt"), b"data for user_abc")
            .await
            .unwrap();
        fs::write(memory_dir.join("other.txt"), b"no user ref")
            .await
            .unwrap();

        let tool = make_tool(ws);
        let result = tool.gdpr_erasure("user_abc").await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("1 files erased"));
        assert!(!memory_dir.join("user_abc.txt").exists());
        assert!(memory_dir.join("other.txt").exists());
    }

    #[tokio::test]
    async fn erasure_disabled_returns_error() {
        let tmp = TempDir::new().unwrap();
        let tool = DataManagementTool::new(
            tmp.path().to_path_buf(),
            90,
            false,
            vec![],
        );
        let result = tool.gdpr_erasure("anyone").await.unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("not enabled"));
    }

    #[test]
    fn human_bytes_formats_correctly() {
        assert_eq!(human_bytes(500), "500 B");
        assert_eq!(human_bytes(1024), "1.0 KB");
        assert_eq!(human_bytes(1_048_576), "1.0 MB");
    }
}
