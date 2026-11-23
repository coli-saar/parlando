//! Resolves the game against which a standalone agent is developed.

use anyhow::{ensure, Context, Result};
use parlando::{
    component::{LoadedGame, WasmMetering},
    deployment::{self, PACKAGE_ZIP_LIMIT},
    GameMetadata,
};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    time::Duration,
};

/// A ZIP locator pinned to an exact game version and checksum.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Dependency {
    pub(super) source: String,
    pub(super) version: semver::Version,
    pub(super) sha256: String,
}

/// A validated game executable and the metadata from the same resolved build.
pub(super) struct ResolvedGame {
    pub(super) game: LoadedGame,
    pub(super) metadata: GameMetadata,
}

/// The operational kind of a project-relative dependency locator.
enum Source {
    Release(Url),
    Archive(PathBuf),
}

impl Source {
    /// Recognizes release URLs and local ZIP paths.
    fn parse(root: &Path, input: &str) -> Result<Self> {
        ensure!(!input.is_empty(), "game dependency source cannot be empty");
        if input.contains("://") {
            let url = Url::parse(input).context("invalid game release URL")?;
            ensure!(
                matches!(url.scheme(), "http" | "https")
                    && url.host_str().is_some()
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.fragment().is_none(),
                "game release must be an HTTP(S) URL without credentials or a fragment"
            );
            return Ok(Self::Release(url));
        }
        let path = root.join(input);
        ensure!(
            !path.is_dir()
                && path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("zip")),
            "game dependency must be a built game ZIP or an HTTP(S) release URL"
        );
        Ok(Self::Archive(path))
    }
}

/// Resolves a user-supplied source and pins the exact artifact used for code generation.
/// Local arguments resolve from the caller's working directory.
pub(super) async fn prepare(
    root: &Path,
    input: &str,
    metering: WasmMetering,
) -> Result<(Dependency, ResolvedGame)> {
    let (source, sha256, game) = match Source::parse(&env::current_dir()?, input)? {
        Source::Release(url) => {
            let archive = download(url.clone()).await?;
            let game = load_snapshot(&archive, metering)?;
            let digest = checksum(&archive);
            cache(root, &digest, &archive)?;
            (url.into(), digest, game)
        }
        Source::Archive(path) => {
            let archive = deployment::read_package_bytes(&path)?;
            let game = load_snapshot(&archive, metering)?;
            (relative_source(root, &path)?, checksum(&archive), game)
        }
    };
    let dependency = Dependency {
        source,
        version: game.metadata.version.clone(),
        sha256,
    };
    Ok((dependency, game))
}

/// Loads the pinned game ZIP, verifying its checksum, game ID, and version.
pub(super) async fn resolve(
    root: &Path,
    game_id: &str,
    dependency: &Dependency,
    metering: WasmMetering,
) -> Result<ResolvedGame> {
    let game = match Source::parse(root, &dependency.source)? {
        Source::Archive(path) => {
            let digest = locked_checksum(dependency)?;
            let archive = deployment::read_package_bytes(&path)?;
            verify_checksum(&archive, digest)
                .with_context(|| format!("checking game archive {}", path.display()))?;
            load_snapshot(&archive, metering)?
        }
        Source::Release(url) => {
            return resolve_release(root, game_id, dependency, url, metering).await;
        }
    };
    verify_identity(&game, game_id, &dependency.version)?;
    Ok(game)
}

/// Resolves a verified cached release or fetches and caches its pinned bytes on a cache miss.
async fn resolve_release(
    root: &Path,
    game_id: &str,
    dependency: &Dependency,
    url: Url,
    metering: WasmMetering,
) -> Result<ResolvedGame> {
    let digest = locked_checksum(dependency)?;
    let path = cache_path(root, digest);
    let (archive, needs_cache) = match path.metadata() {
        Ok(_) => (deployment::read_package_bytes(&path)?, false),
        Err(error) if error.kind() == ErrorKind::NotFound => (download(url).await?, true),
        Err(error) => return Err(error).with_context(|| format!("opening {}", path.display())),
    };
    verify_checksum(&archive, digest)?;
    let game = load_snapshot(&archive, metering)?;
    verify_identity(&game, game_id, &dependency.version)?;
    if needs_cache {
        cache(root, digest, &archive)?;
    }
    Ok(game)
}

/// Checks the archive against the project game ID and pinned version.
fn verify_identity(game: &ResolvedGame, id: &str, version: &semver::Version) -> Result<()> {
    ensure!(
        game.metadata.id == id,
        "expected game {id}, found {}",
        game.metadata.id
    );
    ensure!(
        game.metadata.version == *version,
        "expected game {id} at {version}, found {}; update the game dependency to use that version",
        game.metadata.version
    );
    Ok(())
}

/// Loads the same in-memory ZIP bytes that were checked against the lock.
fn load_snapshot(bytes: &[u8], metering: WasmMetering) -> Result<ResolvedGame> {
    let (game, metadata) = deployment::load_package_bytes(bytes, metering)?;
    Ok(ResolvedGame { game, metadata })
}

/// Fetches a release within the deployment upload limit and a finite request deadline.
async fn download(url: Url) -> Result<Vec<u8>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()?;
    let mut response = client
        .get(url)
        .send()
        .await
        .context("downloading game release")?
        .error_for_status()
        .context("downloading game release")?;
    ensure!(
        response
            .content_length()
            .is_none_or(|size| size <= PACKAGE_ZIP_LIMIT as u64),
        "game release exceeds 64 MiB"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.context("reading game release")? {
        ensure!(
            bytes.len() + chunk.len() <= PACKAGE_ZIP_LIMIT,
            "game release exceeds 64 MiB"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// Computes the lowercase SHA-256 of the ZIP as distributed.
fn checksum(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Validates the stored digest before it can select a path inside the local cache.
fn locked_checksum(dependency: &Dependency) -> Result<&str> {
    let digest = &dependency.sha256;
    ensure!(
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')),
        "game dependency SHA-256 must contain 64 lowercase hexadecimal digits"
    );
    Ok(digest)
}

/// Rejects a changed release or local ZIP before its executable is loaded.
fn verify_checksum(bytes: &[u8], expected: &str) -> Result<()> {
    let actual = checksum(bytes);
    ensure!(
        actual == expected,
        "game archive SHA-256 mismatch (expected {expected}, found {actual})"
    );
    Ok(())
}

/// Locates the project cache entry by the immutable content digest.
fn cache_path(root: &Path, digest: &str) -> PathBuf {
    root.join(".parlando/cache/games")
        .join(format!("{digest}.zip"))
}

/// Publishes only complete, validated archive bytes to the release cache.
fn cache(root: &Path, digest: &str, bytes: &[u8]) -> Result<()> {
    let path = cache_path(root, digest);
    let directory = path.parent().unwrap();
    fs::create_dir_all(directory)?;
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(&path)
        .with_context(|| format!("storing game release in {}", path.display()))?;
    Ok(())
}

/// Makes a source locator portable with its project when their paths share a volume.
fn relative_source(root: &Path, path: &Path) -> Result<String> {
    let root = root
        .canonicalize()
        .with_context(|| format!("locating agent project {}", root.display()))?;
    let filename = path.file_name().context("game ZIP has no filename")?;
    let path = path
        .parent()
        .context("game ZIP has no parent directory")?
        .canonicalize()
        .with_context(|| format!("locating game dependency {}", path.display()))?
        .join(filename);
    let common = root.ancestors().find(|ancestor| path.starts_with(ancestor));
    let relative = match common {
        Some(common) => {
            let mut relative = PathBuf::new();
            for _ in root.strip_prefix(common)?.components() {
                relative.push("..");
            }
            relative.join(path.strip_prefix(common)?)
        }
        None => path,
    };
    relative
        .into_os_string()
        .into_string()
        .map_err(|_| anyhow::anyhow!("game dependency path must be UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &[u8] = include_bytes!("../../agent_experiment/tests/headless.wasm");

    /// Creates a source project and a ZIP using the same validated game component.
    fn game_fixture(directory: &Path) -> Result<(PathBuf, PathBuf)> {
        let source = directory.join("game");
        fs::create_dir(&source)?;
        let manifest = source.join("game.yaml");
        fs::write(
            &manifest,
            "id: test-game\nname: Fixture\nversion: 1.0.0\ncomponent: game.wasm\n",
        )?;
        fs::write(source.join("game.wasm"), FIXTURE)?;
        let archive = directory.join("game.zip");
        deployment::write_package_archive(&source, &archive)?;
        Ok((manifest, archive))
    }

    /// Checks that a cached release can resolve without the origin, and rejects modified bytes.
    #[tokio::test]
    async fn resolves_cached_release_offline_and_checks_its_digest() -> Result<()> {
        let root = tempfile::tempdir()?;
        let (_, archive) = game_fixture(root.path())?;
        let bytes = fs::read(archive)?;
        let digest = checksum(&bytes);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}/game.zip", listener.local_addr()?);
        let origin = tokio::spawn(serve_release(listener, bytes));
        let (dependency, _) = prepare(root.path(), &url, WasmMetering::Fuel).await?;
        origin.await??;
        assert_eq!(dependency.sha256, digest);
        let game = resolve(root.path(), "test-game", &dependency, WasmMetering::Fuel).await?;
        assert_eq!(game.metadata.version, dependency.version);
        fs::write(cache_path(root.path(), &digest), b"changed")?;
        let result = resolve(root.path(), "test-game", &dependency, WasmMetering::Fuel).await;
        assert!(result
            .err()
            .context("expected checksum failure")?
            .to_string()
            .contains("SHA-256 mismatch"));
        Ok(())
    }

    /// Serves one archive and closes its listening socket so a later resolution must use the cache.
    async fn serve_release(listener: tokio::net::TcpListener, archive: Vec<u8>) -> Result<()> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let (stream, _) = listener.accept().await?;
        let mut stream = BufReader::new(stream);
        loop {
            let mut line = String::new();
            if stream.read_line(&mut line).await? == 0 || line == "\r\n" {
                break;
            }
        }
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            archive.len()
        );
        stream.write_all(headers.as_bytes()).await?;
        stream.write_all(&archive).await?;
        stream.shutdown().await?;
        Ok(())
    }

    /// Checks a project-relative ZIP lock and rejects source manifests and directories.
    #[tokio::test]
    async fn locks_local_archive_and_rejects_source_inputs() -> Result<()> {
        let workspace = tempfile::tempdir()?;
        let agent = workspace.path().join("agent");
        fs::create_dir(&agent)?;
        let (manifest, archive) = game_fixture(workspace.path())?;
        for source in [&manifest, manifest.parent().unwrap()] {
            let result = prepare(&agent, source.to_str().unwrap(), WasmMetering::Fuel).await;
            assert!(result
                .err()
                .context("expected source input to be rejected")?
                .to_string()
                .contains("must be a built game ZIP"));
        }
        let (mut dependency, _) =
            prepare(&agent, archive.to_str().unwrap(), WasmMetering::Fuel).await?;
        assert_eq!(dependency.source, "../game.zip");
        assert_eq!(dependency.sha256, checksum(&fs::read(&archive)?));

        let game = resolve(&agent, "test-game", &dependency, WasmMetering::Fuel).await?;
        assert_eq!(game.metadata.version, semver::Version::new(1, 0, 0));
        let result = resolve(&agent, "different-game", &dependency, WasmMetering::Fuel).await;
        assert!(result
            .err()
            .context("expected ID failure")?
            .to_string()
            .contains("expected game different-game"));
        dependency.version = semver::Version::new(2, 0, 0);
        let result = resolve(&agent, "test-game", &dependency, WasmMetering::Fuel).await;
        assert!(result
            .err()
            .context("expected version failure")?
            .to_string()
            .contains("expected game test-game at 2.0.0"));
        Ok(())
    }
}
