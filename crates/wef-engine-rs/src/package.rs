use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::EngineError;

/// A WEF source package loaded from a directory.
#[derive(Debug, Clone)]
pub struct Package {
    root: PathBuf,
    manifest: wef_core::Manifest,
    entry_path: PathBuf,
}

impl Package {
    /// Loads and validates `wef.json` and its declared entry module.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, EngineError> {
        let root = fs::canonicalize(path.as_ref())?;
        if !root.is_dir() {
            return Err(EngineError::InvalidPackage {
                message: format!("package path is not a directory: {}", root.display()),
            });
        }

        let manifest_path = root.join("wef.json");
        let manifest_source = fs::read_to_string(&manifest_path)?;
        let manifest: wef_core::Manifest =
            serde_json::from_str(&manifest_source).map_err(|source| {
                EngineError::ManifestParse {
                    path: manifest_path.clone(),
                    source,
                }
            })?;
        manifest.validate()?;

        let requested_entry = root.join(&manifest.entry);
        let entry_path = fs::canonicalize(&requested_entry).map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                EngineError::InvalidPackage {
                    message: format!("entry module does not exist: {}", requested_entry.display()),
                }
            } else {
                EngineError::Io(source)
            }
        })?;

        if !entry_path.starts_with(&root) {
            return Err(EngineError::InvalidPackage {
                message: format!("entry module escapes package root: {}", manifest.entry),
            });
        }
        if !entry_path.is_file() {
            return Err(EngineError::InvalidPackage {
                message: format!("entry module is not a file: {}", entry_path.display()),
            });
        }

        Ok(Self {
            root,
            manifest,
            entry_path,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn manifest(&self) -> &wef_core::Manifest {
        &self.manifest
    }

    pub fn entry_path(&self) -> &Path {
        &self.entry_path
    }
}
