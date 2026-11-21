//! Defines the upload representation and the immutable package store.

use std::{collections::BTreeMap, fs, io::ErrorKind, path::Path};

use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{component::LoadedGame, GameMetadata};

pub(super) const UPLOAD_LIMIT: usize = 64 * 1024 * 1024;
const FILE_LIMIT: usize = 4096;
const MANIFEST_LIMIT: usize = 64 * 1024;

/// Files in a version-one upload. Keys are portable relative paths and values are base64.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Upload {
    files: BTreeMap<String, String>,
}

/// Developer-controlled package metadata. The host supplies all runtime configuration.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PackageManifest {
    id: String,
    name: String,
    version: semver::Version,
    component: String,
    #[serde(default)]
    client_dist: Option<String>,
}

/// Public result of registering an immutable package revision.
#[derive(Debug, Deserialize, Serialize)]
pub struct Deployment {
    /// Game scope used to store and select this revision.
    pub game: String,
    /// Digest over all canonical paths and decoded file contents.
    pub revision: String,
    /// SHA-256 of the actual game component.
    pub component_sha256: String,
    /// Version declared inside the package.
    pub version: semver::Version,
}

impl Upload {
    /// Reads a built package from disk without following symlinks.
    pub(super) fn read(directory: &Path, manifest: &PackageManifest) -> Result<Self> {
        ensure!(
            fs::symlink_metadata(directory)?.is_dir(),
            "package must be a regular directory"
        );
        let mut files = BTreeMap::new();
        let mut bytes = 0;
        collect(directory, directory, manifest, &mut files, &mut bytes)?;
        Ok(Self { files })
    }

    /// Validates an upload, materializes it privately, and publishes by atomic rename.
    pub(super) fn install(self, store: &Path, game: &str) -> Result<Deployment> {
        ensure!(
            self.files.len() <= FILE_LIMIT,
            "package contains too many files"
        );
        let mut files = BTreeMap::new();
        let mut portable_names = BTreeMap::new();
        let mut hash = Sha256::new();
        hash.update(b"parlando-package-v1\0");
        for (path, encoded) in self.files {
            validate_path(&path)?;
            for prefix in path
                .match_indices('/')
                .map(|(index, _)| &path[..index])
                .chain(std::iter::once(path.as_str()))
            {
                let previous = portable_names
                    .entry(prefix.to_ascii_lowercase())
                    .or_insert_with(|| prefix.to_owned());
                ensure!(previous == prefix, "package contains case-colliding paths");
            }
            let bytes = STANDARD
                .decode(encoded)
                .context("invalid base64 in package")?;
            for value in [path.as_bytes(), &bytes] {
                hash.update((value.len() as u64).to_be_bytes());
                hash.update(value);
            }
            files.insert(path, bytes);
        }
        let manifest =
            PackageManifest::parse(files.get("game.yaml").context("missing game.yaml")?)?;
        ensure!(
            manifest.id == game,
            "package game ID does not match upload scope"
        );
        let component = files
            .get(&manifest.component)
            .context("package is missing its component")?;
        let component_sha256 = format!("{:x}", Sha256::digest(component));
        for path in files.keys() {
            manifest.validate_file(path)?;
        }
        if let Some(client) = &manifest.client_dist {
            ensure!(
                files.contains_key(&format!("{client}/index.html")),
                "package is missing its client index"
            );
        }
        let revision = format!("{:x}", hash.finalize());
        let destination = store.join("packages").join(game).join(&revision);
        let result = Deployment {
            game: game.into(),
            revision,
            version: manifest.version.clone(),
            component_sha256,
        };
        if destination.is_dir() {
            return Ok(result);
        }
        let parent = destination.parent().expect("package game directory");
        fs::create_dir_all(parent)?;
        let staging = parent.join(format!(".upload-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&staging)?;
        let install = (|| -> Result<()> {
            for (path, bytes) in files {
                let target = staging.join(path);
                fs::create_dir_all(target.parent().expect("package file parent"))?;
                use std::io::Write as _;
                let mut file = fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(target)?;
                file.write_all(&bytes)?;
                file.sync_all()?;
            }
            manifest.load(&staging, &result.revision)?;
            sync_directory(&staging)?;
            match fs::rename(&staging, &destination) {
                Ok(()) => fs::File::open(parent)?.sync_all().map_err(Into::into),
                Err(_) if destination.is_dir() => Ok(()),
                Err(error) => Err(error.into()),
            }
        })();
        match fs::remove_dir_all(&staging) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(%error, "could not remove package staging directory"),
        }
        install?;
        Ok(result)
    }
}

impl PackageManifest {
    /// Identifies the exact game scope declared by this package.
    pub(super) fn id(&self) -> &str {
        &self.id
    }

    /// Reports the public version declared by an installed package.
    pub(super) fn version(&self) -> &semver::Version {
        &self.version
    }

    /// Reads and validates package metadata from a committed revision.
    pub(super) fn read(directory: &Path) -> Result<Self> {
        let path = directory.join("game.yaml");
        let metadata = fs::symlink_metadata(&path)?;
        ensure!(metadata.is_file(), "game.yaml must be a regular file");
        ensure!(
            metadata.len() <= MANIFEST_LIMIT as u64,
            "game.yaml exceeds the manifest size limit"
        );
        Self::parse(&fs::read(path)?)
    }

    /// Resolves metadata and client paths against the containing package revision.
    fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() <= MANIFEST_LIMIT,
            "game.yaml exceeds the manifest size limit"
        );
        let manifest: Self = serde_yaml::from_slice(bytes).context("invalid game.yaml")?;
        validate_id(&manifest.id)?;
        validate_path(&manifest.component)?;
        if let Some(client) = &manifest.client_dist {
            validate_path(client)?;
            ensure!(
                !manifest.component.starts_with(&format!("{client}/")),
                "component must be outside the client directory"
            );
        }
        Ok(manifest)
    }

    /// Ensures a file belongs to the manifest, component, or declared participant assets.
    fn validate_file(&self, path: &str) -> Result<()> {
        if path == "game.yaml" || path == self.component {
            return Ok(());
        }
        let asset_path = self
            .client_dist
            .as_ref()
            .and_then(|client| path.strip_prefix(client))
            .and_then(|relative| relative.strip_prefix('/'))
            .with_context(|| format!("unexpected file outside the participant app: {path:?}"))?;
        let namespace = asset_path
            .split('/')
            .next()
            .expect("validated package path");
        ensure!(
            !matches!(
                namespace.to_ascii_lowercase().as_str(),
                "api" | "ws" | "admin" | "health"
            ),
            "client path uses a reserved server namespace"
        );
        Ok(())
    }

    /// Allows only directories on paths to the component or to declared participant assets.
    fn validate_directory(&self, path: &str) -> Result<()> {
        let prefix = format!("{path}/");
        let allowed = self.component.starts_with(&prefix)
            || self.client_dist.as_ref().is_some_and(|client| {
                client == path
                    || client.starts_with(&prefix)
                    || path
                        .strip_prefix(client)
                        .is_some_and(|remainder| remainder.starts_with('/'))
            });
        ensure!(
            allowed,
            "unexpected directory outside the participant app: {path:?}"
        );
        Ok(())
    }

    /// Validates the Wasm ABI on the host and returns the loaded game and recorded identity.
    pub(super) fn load(
        &self,
        directory: &Path,
        revision: &str,
    ) -> Result<(LoadedGame, GameMetadata)> {
        let loaded_game =
            LoadedGame::load(&directory.join(&self.component)).context("invalid game component")?;
        let metadata = GameMetadata {
            id: self.id.clone(),
            name: self.name.clone(),
            version: self.version.clone(),
            build_manifest: serde_json::json!({"component_sha256": loaded_game.digest(), "package_revision": revision}),
        };
        metadata.validate()?;
        Ok((loaded_game, metadata))
    }

    /// Returns the validated package-relative participant application directory if present.
    pub(super) fn client(&self, directory: &Path) -> Option<String> {
        self.client_dist
            .as_ref()
            .map(|client| directory.join(client).to_string_lossy().into_owned())
    }
}

/// Restricts identifiers to single, portable path components.
pub(super) fn validate_id(id: &str) -> Result<()> {
    ensure!(
        !id.is_empty()
            && id.len() <= 64
            && id.as_bytes()[0].is_ascii_alphanumeric()
            && id.bytes().all(|byte| byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || byte == b'-'
                || byte == b'_'),
        "invalid identifier"
    );
    Ok(())
}

/// Validates an exact lowercase SHA-256 revision before constructing a filesystem path.
pub(super) fn validate_revision(revision: &str) -> Result<()> {
    ensure!(
        revision.len() == 64
            && revision
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "invalid package revision"
    );
    Ok(())
}

/// Rejects traversal, ambiguous paths, and non-portable names before writing upload data.
fn validate_path(path: &str) -> Result<()> {
    ensure!(
        path.len() <= 512
            && path.split('/').all(|part| {
                !part.is_empty()
                    && !part.starts_with('.')
                    && !part.ends_with('.')
                    && part.len() <= 128
                    && part
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
            }),
        "invalid package path {path:?}"
    );
    Ok(())
}

/// Flushes the staging directory tree after every file has been synchronized.
fn sync_directory(directory: &Path) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            sync_directory(&entry.path())?;
        }
    }
    fs::File::open(directory)?.sync_all()?;
    Ok(())
}

/// Walks the build output while bounding the number and total size of files read.
fn collect(
    root: &Path,
    directory: &Path,
    manifest: &PackageManifest,
    files: &mut BTreeMap<String, String>,
    bytes: &mut usize,
) -> Result<()> {
    for entry in
        fs::read_dir(directory).with_context(|| format!("reading {}", directory.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let relative = path
            .strip_prefix(root)?
            .iter()
            .map(|part| part.to_str().context("invalid package filename"))
            .collect::<Result<Vec<_>>>()?
            .join("/");
        validate_path(&relative)?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            manifest.validate_directory(&relative)?;
            collect(root, &path, manifest, files, bytes)?;
        } else {
            manifest.validate_file(&relative)?;
            ensure!(
                kind.is_file(),
                "package paths must name regular files and directories"
            );
            ensure!(files.len() < FILE_LIMIT, "package contains too many files");
            *bytes += usize::try_from(entry.metadata()?.len())?;
            ensure!(*bytes <= UPLOAD_LIMIT / 2, "package exceeds upload limit");
            let data = fs::read(&path)?;
            files.insert(relative, STANDARD.encode(data));
        }
    }
    Ok(())
}
