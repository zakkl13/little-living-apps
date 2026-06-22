use std::path::{Path, PathBuf};

use serde_json::Value;
use tempfile::{Builder, TempDir};

use crate::errors::{Error, Result};

/// Temporary on-disk output schema file passed to `codex --output-schema`.
///
/// The underlying temporary directory is kept alive by this struct and cleaned
/// up automatically when dropped.
pub struct OutputSchemaFile {
    _dir: TempDir,
    path: PathBuf,
}

impl OutputSchemaFile {
    /// Returns the filesystem path of the generated schema file.
    ///
    /// # Example
    ///
    /// ```rust
    /// use codex::output_schema_file::create_output_schema_file;
    /// use serde_json::json;
    ///
    /// let file = create_output_schema_file(Some(&json!({"type":"object"})))?
    ///     .expect("schema file should exist");
    /// assert!(file.path().exists());
    /// # Ok::<(), codex::Error>(())
    /// ```
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Creates a temporary JSON schema file for structured output turns.
///
/// Returns `Ok(None)` when no schema is provided.
///
/// # Example
///
/// ```rust
/// use codex::output_schema_file::create_output_schema_file;
/// use serde_json::json;
///
/// let file = create_output_schema_file(Some(&json!({"type":"object"})))?;
/// assert!(file.is_some());
///
/// let absent = create_output_schema_file(None)?;
/// assert!(absent.is_none());
/// # Ok::<(), codex::Error>(())
/// ```
pub fn create_output_schema_file(schema: Option<&Value>) -> Result<Option<OutputSchemaFile>> {
    let Some(schema) = schema else {
        return Ok(None);
    };

    if !schema.is_object() {
        return Err(Error::InvalidOutputSchema(
            "output_schema must be a plain JSON object".to_string(),
        ));
    }

    let dir = Builder::new().prefix("codex-output-schema-").tempdir()?;
    let schema_path = dir.path().join("schema.json");
    std::fs::write(&schema_path, serde_json::to_vec(schema)?)?;

    Ok(Some(OutputSchemaFile {
        _dir: dir,
        path: schema_path,
    }))
}
