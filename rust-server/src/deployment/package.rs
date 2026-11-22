//! Validates ZIP packages and publishes the current build for each hosted game.

use std::{
    collections::BTreeMap,
    fs,
    io::{Cursor, ErrorKind, Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

use crate::{
    component::{LoadedGame, WasmMetering},
    GameMetadata,
};

pub(super) const UPLOAD_LIMIT: usize = 64 * 1024 * 1024;
const EXPANDED_LIMIT: usize = 64 * 1024 * 1024;
const FILE_LIMIT: usize = 4096;
const MANIFEST_LIMIT: usize = 64 * 1024;

/// Fully decoded regular files and the validated manifest for a portable game revision.
pub(super) struct Package {
    files: BTreeMap<String, Vec<u8>>,
    manifest: PackageManifest,
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

/// Identifies the build currently installed for a hosted game.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Deployment {
    /// Game ID that owns this build.
    pub game: String,
    /// Display name declared inside the package.
    pub name: String,
    /// Digest over all canonical paths and decoded file contents.
    pub revision: String,
    /// SHA-256 of the actual game component.
    pub component_sha256: String,
    /// Version declared inside the package.
    pub version: semver::Version,
}

/// Directory entry kind as declared by its path and, when present, its Unix mode.
#[derive(Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    File,
    Directory,
}

impl Package {
    /// Opens an ordinary local archive and bounds its size even if it changes during the read.
    pub(super) fn read_archive(path: &Path) -> Result<(Self, Vec<u8>)> {
        let metadata = fs::metadata(path).with_context(|| format!("opening {}", path.display()))?;
        ensure!(metadata.is_file(), "package must be a ZIP file");
        let file = fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
        ensure!(file.metadata()?.is_file(), "package must be a ZIP file");
        let mut bytes = Vec::new();
        file.take(UPLOAD_LIMIT as u64 + 1).read_to_end(&mut bytes)?;
        Ok((Self::decode(&bytes)?, bytes))
    }

    /// Decodes a single-disk ZIP and rejects ambiguous names, special files, and excess expansion.
    pub(super) fn decode(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() <= UPLOAD_LIMIT,
            "package exceeds the ZIP size limit"
        );
        let (directory_start, entry_count) = central_directory(bytes)?;
        let mut archive = ZipArchive::new(Cursor::new(bytes)).context("invalid package ZIP")?;
        ensure!(
            archive.offset() == 0 && archive.central_directory_start() == directory_start as u64,
            "invalid ZIP central directory location"
        );
        // The ZIP reader indexes by name and otherwise silently replaces an exact duplicate.
        ensure!(
            archive.len() == entry_count,
            "package contains duplicate ZIP paths"
        );

        let mut files = BTreeMap::new();
        let mut names = BTreeMap::new();
        let mut storage = Vec::with_capacity(entry_count);
        let mut directories = Vec::new();
        let mut expanded = 0;
        for index in 0..entry_count {
            let entry = archive.by_index(index).context("invalid ZIP member")?;
            let raw = std::str::from_utf8(entry.name_raw()).context("invalid ZIP filename")?;
            ensure!(raw == entry.name(), "inconsistent ZIP filename");
            let (path, kind) = match raw.strip_suffix('/') {
                Some(path) => (path, EntryKind::Directory),
                None => (raw, EntryKind::File),
            };
            let mode = entry.unix_mode().unwrap_or_default() & 0o170000;
            ensure!(
                mode == 0
                    || (mode == 0o100000 && kind == EntryKind::File)
                    || (mode == 0o040000 && kind == EntryKind::Directory),
                "package ZIP entries must be regular files or directories"
            );
            validate_entry_path(&mut names, path, kind)?;
            storage.push(validate_member_location(bytes, directory_start, &entry)?);

            let remaining = EXPANDED_LIMIT - expanded;
            let limit = match kind {
                EntryKind::Directory => 0,
                EntryKind::File if path == "game.yaml" => remaining.min(MANIFEST_LIMIT),
                EntryKind::File => remaining,
            };
            ensure!(
                entry.size() <= limit as u64,
                "ZIP member exceeds the expanded package size limit"
            );
            let path = path.to_owned();
            let mut data = Vec::new();
            let declared_size = entry.size();
            entry
                .take(declared_size + 1)
                .read_to_end(&mut data)
                .with_context(|| format!("reading ZIP member {path:?}"))?;
            ensure!(
                declared_size == data.len() as u64,
                "ZIP member size is inconsistent"
            );
            expanded += data.len();
            match kind {
                EntryKind::Directory => directories.push(path),
                EntryKind::File => {
                    files.insert(path, data);
                }
            }
        }
        storage.sort_unstable();
        ensure!(
            storage[0].0 == 0 && storage.windows(2).all(|pair| pair[0].1 <= pair[1].0),
            "package contains overlapping ZIP members or unexpected preceding data"
        );
        let package = Self::from_files(files)?;
        for path in directories {
            package.manifest.validate_directory(&path)?;
        }
        Ok(package)
    }

    /// Checks that every build artifact is listed by the manifest and all required files are present.
    fn from_files(files: BTreeMap<String, Vec<u8>>) -> Result<Self> {
        let manifest =
            PackageManifest::parse(files.get("game.yaml").context("missing game.yaml")?)?;
        ensure!(
            files.contains_key(&manifest.component),
            "package is missing its component"
        );
        for path in files.keys() {
            manifest.validate_file(path)?;
        }
        if let Some(client) = &manifest.client_dist {
            ensure!(
                files.contains_key(&format!("{client}/index.html")),
                "package is missing its client index"
            );
        }
        Ok(Self { files, manifest })
    }

    /// Returns the game ID declared by the archive's validated manifest.
    pub(super) fn game(&self) -> &str {
        &self.manifest.id
    }

    /// Computes the canonical revision from sorted paths and bytes, independently of ZIP encoding.
    pub(super) fn revision(&self) -> String {
        let mut hash = Sha256::new();
        hash.update(b"parlando-package-v1\0");
        for (path, bytes) in &self.files {
            for value in [path.as_bytes(), bytes] {
                hash.update((value.len() as u64).to_be_bytes());
                hash.update(value);
            }
        }
        format!("{:x}", hash.finalize())
    }

    /// Loads the component and validates the guest ABI and schema directly from the package files.
    pub(super) fn load(&self, metering: WasmMetering) -> Result<(LoadedGame, GameMetadata)> {
        let game =
            LoadedGame::from_bytes_with_metering(&self.files[&self.manifest.component], metering)
                .context("invalid game component")?;
        self.manifest.describe(game, &self.revision())
    }

    /// Prepares a complete build directory for validation and runtime construction.
    pub(super) fn stage(self, parent: &Path, revision: &str) -> Result<StagedBuild> {
        fs::create_dir_all(parent)?;
        // A failed or interrupted previous attempt has no committed pointer to this revision.
        let directory = parent.join(format!(".build-{revision}"));
        remove_directory(&directory)?;
        fs::create_dir(&directory)?;
        let staged = StagedBuild {
            directory,
            published: false,
        };
        for (path, bytes) in self.files {
            let target = staged.directory.join(path);
            fs::create_dir_all(target.parent().expect("package file parent"))?;
            let mut file = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(target)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        sync_directory(&staged.directory)?;
        fs::File::open(parent)?.sync_all()?;
        fs::File::open(parent.parent().expect("package store"))?.sync_all()?;
        Ok(staged)
    }
}

/// Deletes an unfinished build unless it became the durable current build.
pub(super) struct StagedBuild {
    pub(super) directory: PathBuf,
    published: bool,
}

impl StagedBuild {
    /// Replaces the current pointer once the host has prepared the new runtime.
    /// Reports whether the directory flush succeeded so the caller can prune the old build.
    pub(super) fn publish(mut self, revision: &str) -> Result<bool> {
        let parent = self.directory.parent().expect("package game directory");
        let temporary = parent.join(".current");
        let result = (|| -> Result<()> {
            let mut file = fs::File::create(&temporary)?;
            file.write_all(revision.as_bytes())?;
            file.sync_all()?;
            fs::rename(&temporary, parent.join("current"))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result?;
        self.published = true;
        // After rename, requests must use the new build even if the durability flush fails.
        match fs::File::open(parent).and_then(|directory| directory.sync_all()) {
            Ok(()) => Ok(true),
            Err(error) => {
                tracing::error!(%error, "could not flush current package pointer");
                Ok(false)
            }
        }
    }
}

impl Drop for StagedBuild {
    /// Cleans up candidate data when validation, runtime construction, or publishing fails.
    fn drop(&mut self) {
        if !self.published {
            if let Err(error) = remove_directory(&self.directory) {
                tracing::warn!(%error, "could not remove unfinished game build");
            }
        }
    }
}

/// Loads the committed pointer, rejects the former versioned store, and prunes staging data.
pub(super) fn recover_build(parent: &Path) -> Result<Option<(String, PathBuf)>> {
    let pointer = parent.join("current");
    let revision = match fs::read_to_string(&pointer) {
        Ok(revision) => {
            validate_revision(&revision)?;
            Some(revision)
        }
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let current = revision.map(|revision| {
        let directory = parent.join(format!(".build-{revision}"));
        (revision, directory)
    });
    if let Some((_, directory)) = &current {
        ensure!(directory.is_dir(), "the current game build is missing");
    }
    // A process may have crashed just after renaming the pointer. Flush that rename
    // before removing any directory that a previous pointer might still reference.
    fs::File::open(parent)?.sync_all()?;
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_str().context("invalid stored build name")?;
        if name == "current" {
            continue;
        }
        if name == ".current" {
            fs::remove_file(entry.path())?;
            continue;
        }
        ensure!(
            name.strip_prefix(".build-")
                .is_some_and(|revision| validate_revision(revision).is_ok()),
            "the deployment store contains legacy packages and requires offline conversion"
        );
        if current
            .as_ref()
            .is_none_or(|(_, directory)| *directory != entry.path())
        {
            remove_directory(&entry.path())?;
        }
    }
    fs::File::open(parent)?.sync_all()?;
    Ok(current)
}

/// Removes build data after replacement or cleanup, including an already absent directory.
fn remove_directory(directory: &Path) -> Result<()> {
    match fs::remove_dir_all(directory) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

impl PackageManifest {
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
        ensure!(
            manifest.component != "game.yaml",
            "component cannot be game.yaml"
        );
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
        metering: WasmMetering,
    ) -> Result<(LoadedGame, GameMetadata)> {
        let loaded_game =
            LoadedGame::load_with_metering(&directory.join(&self.component), metering)
                .context("invalid game component")?;
        self.describe(loaded_game, revision)
    }

    /// Associates a checked component with the validated identity of its source package.
    fn describe(
        &self,
        loaded_game: LoadedGame,
        revision: &str,
    ) -> Result<(LoadedGame, GameMetadata)> {
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
        "IDs must start with a lowercase ASCII letter or digit and contain at most 64 characters. Use lowercase ASCII letters, digits, hyphens, or underscores. For example, beacon-test."
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

/// Validates and writes a deterministic ZIP of a build staging directory without replacing an existing file.
pub fn write_package_archive(directory: &Path, archive: &Path) -> Result<()> {
    ensure!(
        fs::symlink_metadata(directory)?.is_dir(),
        "package source must be a regular directory"
    );
    let manifest = PackageManifest::read(directory)?;
    let mut files = BTreeMap::new();
    let mut names = BTreeMap::new();
    let mut expanded = 0;
    collect(
        directory,
        directory,
        &manifest,
        &mut files,
        &mut names,
        &mut expanded,
    )?;
    let package = Package::from_files(files)?;

    let parent = archive
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut output = tempfile::NamedTempFile::new_in(parent)?;
    let mut writer = ZipWriter::new(output.as_file_mut());
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, data) in package.files {
        writer.start_file(name, options)?;
        writer.write_all(&data)?;
    }
    let file = writer.finish()?;
    ensure!(
        file.metadata()?.len() <= UPLOAD_LIMIT as u64,
        "package exceeds the ZIP size limit"
    );
    file.sync_all()?;
    output.persist_noclobber(archive)?;
    Ok(())
}

/// Locates the standard single-disk end record before the ZIP reader allocates its entry index.
fn central_directory(bytes: &[u8]) -> Result<(usize, usize)> {
    let search_start = bytes.len().saturating_sub(22 + u16::MAX as usize);
    let end = bytes
        .get(search_start..)
        .context("invalid ZIP length")?
        .windows(4)
        .enumerate()
        .rev()
        .find(|&(offset, signature)| {
            let end = &bytes[search_start + offset..];
            signature == b"PK\x05\x06"
                && zip_u16(end, 20).is_ok_and(|comment| end.len() == 22 + comment)
        })
        .map(|(offset, _)| search_start + offset)
        .context("missing ZIP end record")?;
    let record = &bytes[end..];
    let count = zip_u16(record, 10)?;
    ensure!(count > 0, "package ZIP contains no entries");
    ensure!(count <= FILE_LIMIT, "package contains too many ZIP entries");
    ensure!(
        zip_u16(record, 4)? == 0 && zip_u16(record, 6)? == 0 && zip_u16(record, 8)? == count,
        "package must be a single-disk ZIP"
    );
    let directory_size = u32::from_le_bytes(record[12..16].try_into()?) as usize;
    let directory_start = u32::from_le_bytes(record[16..20].try_into()?) as usize;
    ensure!(
        directory_start.checked_add(directory_size) == Some(end),
        "invalid ZIP directory size (ZIP64 archives are unsupported)"
    );
    let mut remaining = &bytes[directory_start..end];
    for _ in 0..count {
        ensure!(
            remaining.starts_with(b"PK\x01\x02"),
            "invalid ZIP central member"
        );
        let name = zip_u16(remaining, 28)?;
        let extra = zip_u16(remaining, 30)?;
        let comment = zip_u16(remaining, 32)?;
        remaining = remaining
            .get(46 + name + extra + comment..)
            .context("truncated ZIP central member")?;
    }
    ensure!(remaining.is_empty(), "invalid ZIP directory entry count");
    Ok((directory_start, count))
}

/// Checks that each member's local header agrees with its central metadata and fits within the ZIP.
fn validate_member_location(
    bytes: &[u8],
    directory_start: usize,
    entry: &zip::read::ZipFile<'_, Cursor<&[u8]>>,
) -> Result<(usize, usize)> {
    let start = usize::try_from(entry.header_start())?;
    let central = usize::try_from(entry.central_header_start())?;
    let data = usize::try_from(entry.data_start().context("missing ZIP data location")?)?;
    let compressed = usize::try_from(entry.compressed_size())?;
    let end = data
        .checked_add(compressed)
        .context("invalid ZIP data size")?;
    let header = bytes.get(start..data).context("invalid ZIP local header")?;
    let metadata = bytes
        .get(central..)
        .context("invalid ZIP central member location")?;
    let name = zip_u16(header, 26)?;
    let extra = zip_u16(header, 28)?;
    ensure!(
        header.starts_with(b"PK\x03\x04")
            && header.get(30..30 + name) == Some(entry.name_raw())
            && header.len() == 30 + name + extra
            && header.get(6..10) == metadata.get(8..12)
            && end <= directory_start,
        "inconsistent ZIP local member header"
    );
    // Streaming ZIPs declare these fields in a data descriptor after the payload.
    if zip_u16(header, 6)? & (1 << 3) == 0 {
        ensure!(
            header.get(14..26) == metadata.get(16..28),
            "inconsistent ZIP local member sizes or checksum"
        );
    }
    Ok((start, end))
}

/// Reads a fixed-width little-endian field from a ZIP header with bounds checking.
fn zip_u16(bytes: &[u8], offset: usize) -> Result<usize> {
    let value = bytes
        .get(offset..offset + 2)
        .context("truncated ZIP header")?;
    Ok(u16::from_le_bytes(value.try_into()?) as usize)
}

/// Checks spelling and file types for every explicit member and implicit parent path.
fn validate_entry_path(
    names: &mut BTreeMap<String, (String, EntryKind)>,
    path: &str,
    kind: EntryKind,
) -> Result<()> {
    validate_path(path)?;
    let parents = path
        .match_indices('/')
        .map(|(index, _)| (&path[..index], EntryKind::Directory));
    for (prefix, expected) in parents.chain(std::iter::once((path, kind))) {
        let (spelling, actual) = names
            .entry(prefix.to_ascii_lowercase())
            .or_insert_with(|| (prefix.to_owned(), expected));
        ensure!(
            spelling == prefix && *actual == expected,
            "package contains conflicting or case-colliding paths"
        );
    }
    Ok(())
}

/// Reads the declared build tree while bounding its paths, entry count, and expanded size.
fn collect(
    root: &Path,
    directory: &Path,
    manifest: &PackageManifest,
    files: &mut BTreeMap<String, Vec<u8>>,
    names: &mut BTreeMap<String, (String, EntryKind)>,
    expanded: &mut usize,
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
        let kind = entry.file_type()?;
        if kind.is_dir() {
            validate_entry_path(names, &relative, EntryKind::Directory)?;
            manifest.validate_directory(&relative)?;
            ensure!(names.len() <= FILE_LIMIT, "package contains too many paths");
            collect(root, &path, manifest, files, names, expanded)?;
        } else {
            validate_entry_path(names, &relative, EntryKind::File)?;
            manifest.validate_file(&relative)?;
            ensure!(names.len() <= FILE_LIMIT, "package contains too many paths");
            ensure!(
                kind.is_file(),
                "package paths must name regular files and directories"
            );
            let mut data = Vec::new();
            let remaining = EXPANDED_LIMIT - *expanded;
            fs::File::open(&path)?
                .take(remaining as u64 + 1)
                .read_to_end(&mut data)?;
            ensure!(
                data.len() <= remaining,
                "package exceeds the expanded size limit"
            );
            *expanded += data.len();
            files.insert(relative, data);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes a small standalone archive for exercising member metadata rejection.
    fn archive(write: impl FnOnce(&mut ZipWriter<Cursor<Vec<u8>>>)) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        write(&mut writer);
        writer.finish().unwrap().into_inner()
    }

    /// Restores only the committed build when a publication was interrupted before the rename.
    #[test]
    fn recovery_keeps_committed_build_and_prunes_interrupted_candidate() {
        let store = tempfile::tempdir().unwrap();
        let committed = "a".repeat(64);
        let candidate = "b".repeat(64);
        let committed_dir = store.path().join(format!(".build-{committed}"));
        let candidate_dir = store.path().join(format!(".build-{candidate}"));
        fs::create_dir(&committed_dir).unwrap();
        fs::create_dir(&candidate_dir).unwrap();
        fs::write(store.path().join("current"), &committed).unwrap();
        fs::write(store.path().join(".current"), candidate).unwrap();

        assert_eq!(
            recover_build(store.path()).unwrap(),
            Some((committed, committed_dir.clone()))
        );
        assert!(committed_dir.is_dir());
        assert!(!candidate_dir.exists());
        assert!(!store.path().join(".current").exists());
    }

    /// Rejects a symlink before any link target bytes are read or materialized.
    #[test]
    fn zip_rejects_special_entries() {
        let bytes = archive(|writer| {
            writer
                .add_symlink("game.yaml", "outside", SimpleFileOptions::default())
                .unwrap();
        });
        let error = Package::decode(&bytes).err().unwrap();
        assert!(error.to_string().contains("regular files or directories"));
    }

    /// Applies the manifest limit before decompression.
    #[test]
    fn zip_bounds_manifest_expansion() {
        let bytes = archive(|writer| {
            writer
                .start_file("game.yaml", SimpleFileOptions::default())
                .unwrap();
            writer.write_all(&vec![b'x'; MANIFEST_LIMIT + 1]).unwrap();
        });
        let error = Package::decode(&bytes).err().unwrap();
        assert!(error.to_string().contains("expanded package size limit"));
    }

    /// Verifies a member's checksum instead of accepting corrupt contents from a valid ZIP directory.
    #[test]
    fn zip_rejects_bad_checksum() {
        let mut bytes = archive(|writer| {
            let options =
                SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            writer.start_file("game.yaml", options).unwrap();
            writer.write_all(b"manifest contents").unwrap();
        });
        let position = bytes
            .windows(17)
            .position(|bytes| bytes == b"manifest contents")
            .unwrap();
        bytes[position] = b'M';
        let error = Package::decode(&bytes).err().unwrap();
        assert!(format!("{error:#}").contains("checksum"));
    }
}
