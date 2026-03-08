//! Backup and restore tool for ZeroClaw workspace data.
//!
//! Supports creating timestamped backups of config/memory/audit/knowledge,
//! restoring from a named backup (with explicit confirmation), listing
//! available backups, pruning old backups, and verifying backup integrity
//! via SHA-256 checksums stored in each backup's `manifest.json`.

use super::traits::{Tool, ToolResult};
use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio::fs;

/// Manifest stored alongside each backup for integrity verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupManifest {
    /// Backup identifier (timestamp-based).
    pub id: String,
    /// ISO 8601 creation timestamp.
    pub created_at: String,
    /// Files included with their relative paths and SHA-256 hex checksums.
    pub files: Vec<BackupFileEntry>,
}

/// Single file entry inside a backup manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupFileEntry {
    /// Relative path within the backup directory.
    pub path: String,
    /// SHA-256 hex digest of the file contents.
    pub sha256: String,
    /// File size in bytes.
    pub size: u64,
}

/// BackupTool provides backup/restore capabilities for the ZeroClaw workspace.
pub struct BackupTool {
    workspace_dir: PathBuf,
    include_dirs: Vec<String>,
    max_keep: usize,
}

impl BackupTool {
    pub fn new(workspace_dir: PathBuf, include_dirs: Vec<String>, max_keep: usize) -> Self {
        Self {
            workspace_dir,
            include_dirs,
            max_keep,
        }
    }

    fn backups_dir(&self) -> PathBuf {
        self.workspace_dir.join("backups")
    }

    async fn create_backup(&self) -> anyhow::Result<ToolResult> {
        let timestamp = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let backup_id = format!("backup-{timestamp}");
        let backup_dir = self.backups_dir().join(&backup_id);
        fs::create_dir_all(&backup_dir).await?;

        let mut entries = Vec::new();

        for dir_name in &self.include_dirs {
            let source_dir = self.workspace_dir.join(dir_name);
            if !source_dir.exists() {
                continue;
            }
            let dest_dir = backup_dir.join(dir_name);
            copy_dir_recursive(&source_dir, &dest_dir, dir_name, &mut entries).await?;
        }

        let manifest = BackupManifest {
            id: backup_id.clone(),
            created_at: Utc::now().to_rfc3339(),
            files: entries,
        };

        let manifest_json = serde_json::to_string_pretty(&manifest)?;
        fs::write(backup_dir.join("manifest.json"), &manifest_json).await?;

        Ok(ToolResult {
            success: true,
            output: format!(
                "Backup '{}' created with {} files.",
                backup_id,
                manifest.files.len()
            ),
            error: None,
        })
    }

    async fn list_backups(&self) -> anyhow::Result<ToolResult> {
        let backups_dir = self.backups_dir();
        if !backups_dir.exists() {
            return Ok(ToolResult {
                success: true,
                output: "No backups found.".to_string(),
                error: None,
            });
        }

        let mut entries = Vec::new();
        let mut read_dir = fs::read_dir(&backups_dir).await?;
        while let Some(entry) = read_dir.next_entry().await? {
            let path = entry.path();
            if path.is_dir() {
                let manifest_path = path.join("manifest.json");
                if manifest_path.exists() {
                    let manifest_data = fs::read_to_string(&manifest_path).await?;
                    if let Ok(manifest) =
                        serde_json::from_str::<BackupManifest>(&manifest_data)
                    {
                        entries.push(format!(
                            "{} (created: {}, files: {})",
                            manifest.id, manifest.created_at, manifest.files.len()
                        ));
                    }
                }
            }
        }

        entries.sort();
        if entries.is_empty() {
            return Ok(ToolResult {
                success: true,
                output: "No backups found.".to_string(),
                error: None,
            });
        }

        Ok(ToolResult {
            success: true,
            output: format!("Available backups:\n{}", entries.join("\n")),
            error: None,
        })
    }

    async fn verify_backup(&self, backup_id: &str) -> anyhow::Result<ToolResult> {
        let backup_dir = self.backups_dir().join(backup_id);
        let manifest_path = backup_dir.join("manifest.json");
        if !manifest_path.exists() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Backup '{backup_id}' not found.")),
            });
        }

        let manifest_data = fs::read_to_string(&manifest_path).await?;
        let manifest: BackupManifest = serde_json::from_str(&manifest_data)?;

        let mut ok_count = 0usize;
        let mut fail_count = 0usize;
        let mut failures = Vec::new();

        for entry in &manifest.files {
            let file_path = backup_dir.join(&entry.path);
            match fs::read(&file_path).await {
                Ok(data) => {
                    let hash = hex_sha256(&data);
                    if hash == entry.sha256 {
                        ok_count += 1;
                    } else {
                        fail_count += 1;
                        failures.push(format!("  MISMATCH: {}", entry.path));
                    }
                }
                Err(_) => {
                    fail_count += 1;
                    failures.push(format!("  MISSING: {}", entry.path));
                }
            }
        }

        let status = if fail_count == 0 {
            format!("Backup '{backup_id}' verified: {ok_count} files OK.")
        } else {
            format!(
                "Backup '{backup_id}' verification: {ok_count} OK, {fail_count} FAILED.\n{}",
                failures.join("\n")
            )
        };

        Ok(ToolResult {
            success: fail_count == 0,
            output: status,
            error: None,
        })
    }

    async fn restore_backup(
        &self,
        backup_id: &str,
        confirmed: bool,
    ) -> anyhow::Result<ToolResult> {
        if !confirmed {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "Restore requires explicit confirmation. Re-run with confirm=true to restore backup '{backup_id}'."
                )),
            });
        }

        let backup_dir = self.backups_dir().join(backup_id);
        let manifest_path = backup_dir.join("manifest.json");
        if !manifest_path.exists() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Backup '{backup_id}' not found.")),
            });
        }

        let manifest_data = fs::read_to_string(&manifest_path).await?;
        let manifest: BackupManifest = serde_json::from_str(&manifest_data)?;

        let mut restored = 0usize;
        for entry in &manifest.files {
            let src = backup_dir.join(&entry.path);
            let dest = self.workspace_dir.join(&entry.path);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).await?;
            }
            if src.exists() {
                fs::copy(&src, &dest).await?;
                restored += 1;
            }
        }

        Ok(ToolResult {
            success: true,
            output: format!(
                "Restored {restored}/{} files from backup '{backup_id}'.",
                manifest.files.len()
            ),
            error: None,
        })
    }

    async fn prune_backups(&self, keep: Option<usize>) -> anyhow::Result<ToolResult> {
        let keep = keep.unwrap_or(self.max_keep);
        let backups_dir = self.backups_dir();
        if !backups_dir.exists() {
            return Ok(ToolResult {
                success: true,
                output: "No backups to prune.".to_string(),
                error: None,
            });
        }

        let mut backups: Vec<(String, PathBuf)> = Vec::new();
        let mut read_dir = fs::read_dir(&backups_dir).await?;
        while let Some(entry) = read_dir.next_entry().await? {
            let path = entry.path();
            if path.is_dir() && path.join("manifest.json").exists() {
                let name = entry.file_name().to_string_lossy().to_string();
                backups.push((name, path));
            }
        }

        backups.sort_by(|a, b| a.0.cmp(&b.0));

        if backups.len() <= keep {
            return Ok(ToolResult {
                success: true,
                output: format!(
                    "No pruning needed: {} backups (keep={keep}).",
                    backups.len()
                ),
                error: None,
            });
        }

        let to_remove = backups.len() - keep;
        let mut removed = 0usize;
        for (name, path) in backups.iter().take(to_remove) {
            if let Err(error) = fs::remove_dir_all(path).await {
                tracing::warn!("Failed to remove backup '{name}': {error}");
            } else {
                removed += 1;
            }
        }

        Ok(ToolResult {
            success: true,
            output: format!("Pruned {removed} backups, kept {keep}."),
            error: None,
        })
    }
}

#[async_trait]
impl Tool for BackupTool {
    fn name(&self) -> &str {
        "backup"
    }

    fn description(&self) -> &str {
        "Create, restore, list, prune, or verify backups of ZeroClaw workspace data (config, memory, audit, knowledge). Restore always requires explicit confirmation."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["create", "restore", "list", "prune", "verify"],
                    "description": "Backup action to perform"
                },
                "backup_id": {
                    "type": "string",
                    "description": "Backup identifier (required for restore/verify)"
                },
                "confirm": {
                    "type": "boolean",
                    "description": "Explicit confirmation flag (required for restore)"
                },
                "keep": {
                    "type": "integer",
                    "description": "Number of backups to keep during prune (default: max_keep from config)"
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
            "create" => self.create_backup().await,
            "list" => self.list_backups().await,
            "verify" => {
                let backup_id = args
                    .get("backup_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing 'backup_id' for verify"))?;
                self.verify_backup(backup_id).await
            }
            "restore" => {
                let backup_id = args
                    .get("backup_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing 'backup_id' for restore"))?;
                let confirmed = args
                    .get("confirm")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                self.restore_backup(backup_id, confirmed).await
            }
            "prune" => {
                let keep = args
                    .get("keep")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize);
                self.prune_backups(keep).await
            }
            other => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Unknown backup action: '{other}'")),
            }),
        }
    }
}

/// Recursively copy a directory and collect file entries with checksums.
async fn copy_dir_recursive(
    src: &Path,
    dest: &Path,
    prefix: &str,
    entries: &mut Vec<BackupFileEntry>,
) -> anyhow::Result<()> {
    fs::create_dir_all(dest).await?;
    let mut read_dir = fs::read_dir(src).await?;
    while let Some(entry) = read_dir.next_entry().await? {
        let file_type = entry.file_type().await?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        let rel_path = format!("{prefix}/{name_str}");
        let src_path = entry.path();
        let dest_path = dest.join(&name);

        if file_type.is_dir() {
            Box::pin(copy_dir_recursive(&src_path, &dest_path, &rel_path, entries)).await?;
        } else if file_type.is_file() {
            let data = fs::read(&src_path).await?;
            let hash = hex_sha256(&data);
            let size = data.len() as u64;
            fs::write(&dest_path, &data).await?;
            entries.push(BackupFileEntry {
                path: rel_path,
                sha256: hash,
                size,
            });
        }
    }
    Ok(())
}

/// Compute hex-encoded SHA-256 digest.
pub fn hex_sha256(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn manifest_generation_includes_all_files() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().to_path_buf();

        let config_dir = ws.join("config");
        fs::create_dir_all(&config_dir).await.unwrap();
        fs::write(config_dir.join("config.toml"), b"key = true")
            .await
            .unwrap();

        let memory_dir = ws.join("memory");
        fs::create_dir_all(&memory_dir).await.unwrap();
        fs::write(memory_dir.join("notes.md"), b"# Notes")
            .await
            .unwrap();

        let tool = BackupTool::new(
            ws.clone(),
            vec!["config".to_string(), "memory".to_string()],
            5,
        );

        let result = tool.create_backup().await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("2 files"));

        let backups_dir = ws.join("backups");
        let mut read_dir = fs::read_dir(&backups_dir).await.unwrap();
        let backup_entry = read_dir.next_entry().await.unwrap().unwrap();
        let manifest_path = backup_entry.path().join("manifest.json");
        assert!(manifest_path.exists());

        let manifest_data = fs::read_to_string(&manifest_path).await.unwrap();
        let manifest: BackupManifest = serde_json::from_str(&manifest_data).unwrap();
        assert_eq!(manifest.files.len(), 2);
    }

    #[tokio::test]
    async fn checksum_verification_passes_on_intact_backup() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().to_path_buf();

        let config_dir = ws.join("config");
        fs::create_dir_all(&config_dir).await.unwrap();
        fs::write(config_dir.join("test.txt"), b"hello")
            .await
            .unwrap();

        let tool = BackupTool::new(ws.clone(), vec!["config".to_string()], 5);
        tool.create_backup().await.unwrap();

        let mut read_dir = fs::read_dir(ws.join("backups")).await.unwrap();
        let entry = read_dir.next_entry().await.unwrap().unwrap();
        let backup_id = entry.file_name().to_string_lossy().to_string();

        let result = tool.verify_backup(&backup_id).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("1 files OK"));
    }

    #[tokio::test]
    async fn checksum_verification_fails_on_corrupted_backup() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().to_path_buf();

        let config_dir = ws.join("config");
        fs::create_dir_all(&config_dir).await.unwrap();
        fs::write(config_dir.join("test.txt"), b"hello")
            .await
            .unwrap();

        let tool = BackupTool::new(ws.clone(), vec!["config".to_string()], 5);
        tool.create_backup().await.unwrap();

        let mut read_dir = fs::read_dir(ws.join("backups")).await.unwrap();
        let entry = read_dir.next_entry().await.unwrap().unwrap();
        let backup_id = entry.file_name().to_string_lossy().to_string();
        let corrupt_path = entry.path().join("config/test.txt");
        fs::write(&corrupt_path, b"corrupted").await.unwrap();

        let result = tool.verify_backup(&backup_id).await.unwrap();
        assert!(!result.success);
        assert!(result.output.contains("MISMATCH"));
    }

    #[tokio::test]
    async fn restore_requires_explicit_confirmation() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().to_path_buf();

        let tool = BackupTool::new(ws, vec![], 5);
        let result = tool.restore_backup("any-id", false).await.unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("confirmation"));
    }

    #[tokio::test]
    async fn prune_removes_oldest_backups() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().to_path_buf();

        let config_dir = ws.join("config");
        fs::create_dir_all(&config_dir).await.unwrap();
        fs::write(config_dir.join("f.txt"), b"data").await.unwrap();

        let tool = BackupTool::new(ws.clone(), vec!["config".to_string()], 1);

        tool.create_backup().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        tool.create_backup().await.unwrap();

        let result = tool.prune_backups(Some(1)).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("Pruned 1"));

        let mut count = 0;
        let mut rd = fs::read_dir(ws.join("backups")).await.unwrap();
        while rd.next_entry().await.unwrap().is_some() {
            count += 1;
        }
        assert_eq!(count, 1);
    }

    #[test]
    fn hex_sha256_is_deterministic() {
        let hash1 = hex_sha256(b"test data");
        let hash2 = hex_sha256(b"test data");
        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 64);
    }
}
