//! Resolve deployment targets and read a per-origin, per-game upload credential.
//! Only aliases and public origins belong in the configuration file.

use std::{
    collections::BTreeMap,
    env, fs,
    io::{self, IsTerminal, Read},
    net::IpAddr,
    path::PathBuf,
};

use anyhow::{ensure, Context, Result};

use super::{package::validate_id, validate_token};

#[cfg(any(target_os = "macos", target_os = "linux"))]
const SERVICE: &str = "org.coli-saar.parlando.deploy";

/// Resolves a URL or an alias from the user's `parlando/servers.yaml` configuration.
/// Returns a canonical origin suitable for both credential scoping and HTTP calls.
pub fn resolve_host(host_or_alias: &str) -> Result<String> {
    if host_or_alias.contains("://") {
        return origin(host_or_alias);
    }
    validate_id(host_or_alias).context("invalid server alias")?;
    let config = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .context("cannot locate server alias configuration")?
        .join("parlando/servers.yaml");
    let aliases: BTreeMap<String, String> = serde_yaml::from_slice(
        &fs::read(&config).with_context(|| format!("reading {}", config.display()))?,
    )
    .context("invalid servers.yaml alias mapping")?;
    origin(aliases.get(host_or_alias).context("unknown server alias")?)
}

/// Validates the target before looking up secrets or building a request URL.
pub(super) fn origin(value: &str) -> Result<String> {
    let url = reqwest::Url::parse(value).map_err(|_| anyhow::anyhow!("invalid server origin"))?;
    let host = url.host_str().context("server origin needs a hostname")?;
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    ensure!(
        url.scheme() == "https" || (url.scheme() == "http" && loopback),
        "server origin requires HTTPS or loopback HTTP"
    );
    ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && url.path() == "/",
        "supply an origin without credentials, path, query, or fragment"
    );
    Ok(url.origin().ascii_serialization())
}

/// Associates a token with exactly one canonical origin and game.
fn account(origin: &str, game_id: &str) -> Result<String> {
    validate_id(game_id)?;
    Ok(format!("{origin}/{game_id}"))
}

/// Returns the upload credential for a single server and game. A token injected by
/// CI must be paired with its intended origin in `PARLANDO_DEPLOY_ORIGIN`.
pub fn upload_token(server_origin: &str, game_id: &str) -> Result<String> {
    let server_origin = origin(server_origin)?;
    let account = account(&server_origin, game_id)?;
    match env::var("PARLANDO_DEPLOY_TOKEN") {
        Ok(token) => {
            let expected = env::var("PARLANDO_DEPLOY_ORIGIN")
                .context("pair PARLANDO_DEPLOY_TOKEN with PARLANDO_DEPLOY_ORIGIN")?;
            ensure!(
                origin(&expected)? == server_origin,
                "upload credential belongs to another server"
            );
            validate_token(&token)?;
            Ok(token)
        }
        Err(env::VarError::NotPresent) => read_keyring(&account),
        Err(_) => anyhow::bail!("upload credential contains invalid text"),
    }
}

/// Reads one token from a pipe and saves it in the OS credential store for the selected game.
/// Rejects interactive stdin to avoid displaying a secret in terminal echo or shell history.
pub fn login(server: &str, game_id: &str) -> Result<()> {
    let account = account(&resolve_host(server)?, game_id)?;
    let stdin = io::stdin();
    ensure!(
        !stdin.is_terminal(),
        "pipe the upload credential from your secret manager to stdin"
    );
    let mut input = String::new();
    stdin
        .lock()
        .take(8193)
        .read_to_string(&mut input)
        .context("reading upload credential")?;
    let token = input.trim_end_matches(['\r', '\n']);
    ensure!(input.len() <= 8192, "credential too long");
    validate_token(token)?;
    write_keyring(&account, token)
}

/// Reads this account's generic password using the native macOS Keychain API.
#[cfg(target_os = "macos")]
fn read_keyring(account: &str) -> Result<String> {
    let bytes = security_framework::passwords::get_generic_password(SERVICE, account)
        .context("reading Parlando upload credential from Keychain")?;
    String::from_utf8(bytes).context("invalid stored credential")
}

/// Updates the account in the native macOS Keychain without putting secrets in argv.
#[cfg(target_os = "macos")]
fn write_keyring(account: &str, token: &str) -> Result<()> {
    security_framework::passwords::set_generic_password(SERVICE, account, token.as_bytes())
        .context("storing Parlando upload credential in Keychain")
}

/// Queries the desktop Secret Service using the libsecret CLI.
#[cfg(target_os = "linux")]
fn read_keyring(account: &str) -> Result<String> {
    let output = std::process::Command::new("secret-tool")
        .args(["lookup", "service", SERVICE, "account", account])
        .stdin(std::process::Stdio::null())
        .output()
        .context("reading upload credential using secret-tool")?;
    ensure!(
        output.status.success(),
        "no upload credential available from Secret Service"
    );
    let token = String::from_utf8(output.stdout).context("invalid stored credential")?;
    Ok(token.trim_end_matches(['\r', '\n']).to_owned())
}

/// Sends the secret through a private pipe to libsecret.
#[cfg(target_os = "linux")]
fn write_keyring(account: &str, token: &str) -> Result<()> {
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    let mut child = Command::new("secret-tool")
        .args([
            "store",
            "--label",
            "Parlando game upload",
            "service",
            SERVICE,
            "account",
            account,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .context("starting secret-tool")?;
    let result = child
        .stdin
        .take()
        .context("opening secret-tool input")?
        .write_all(token.as_bytes());
    let status = child.wait()?;
    result?;
    ensure!(
        status.success(),
        "Secret Service could not store the credential"
    );
    Ok(())
}

/// Reports the supported unattended credential path on platforms without a keyring adapter.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn read_keyring(_: &str) -> Result<String> {
    anyhow::bail!("set PARLANDO_DEPLOY_ORIGIN and PARLANDO_DEPLOY_TOKEN on this platform")
}

/// Reports how to inject a credential on platforms without a keyring adapter.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn write_keyring(_: &str, _: &str) -> Result<()> {
    anyhow::bail!("set PARLANDO_DEPLOY_ORIGIN and PARLANDO_DEPLOY_TOKEN on this platform")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Credential lookup uses the same normalized origin that receives the request.
    #[test]
    fn restricts_and_normalizes_credential_origins() {
        assert_eq!(
            origin("https://EXAMPLE.com:443/").unwrap(),
            "https://example.com"
        );
        assert!(origin("http://[::1]:8000").is_ok());
        for url in [
            "http://example.com",
            "file:///tmp",
            "https://alice:pw@example.com",
            "https://example.com/lab",
            "https://example.com/?query",
            "https://example.com/#hash",
        ] {
            assert!(origin(url).is_err(), "{url}");
        }
    }
}
