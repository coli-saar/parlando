//! Resolve deployment targets and sign in to a deployment server using its dashboard accounts.
//! Only aliases and public origins belong in the configuration file.

use std::{collections::BTreeMap, env, fs, net::IpAddr, path::PathBuf};

use anyhow::{ensure, Context, Result};

use super::{package::validate_id, validate_token};

#[cfg(any(target_os = "macos", target_os = "linux"))]
const SERVICE: &str = "org.coli-saar.parlando.deploy";

/// Resolves a URL or an alias from the user's `~/.parlando/servers.yaml` configuration.
/// Returns a canonical origin suitable for both credential scoping and HTTP calls.
pub fn resolve_host(host_or_alias: &str) -> Result<String> {
    if host_or_alias.contains("://") {
        return origin(host_or_alias);
    }
    validate_id(host_or_alias).context("invalid server alias")?;
    let config = env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .context("Set HOME to locate ~/.parlando/servers.yaml.")?
        .join(".parlando/servers.yaml");
    let contents = fs::read(&config).with_context(|| {
        format!(
            "Could not read {}. Specify a server URL or define an alias in this file.",
            config.display()
        )
    })?;
    let aliases: BTreeMap<String, String> = serde_yaml::from_slice(&contents)
        .with_context(|| format!("Invalid alias mapping in {}", config.display()))?;
    let lookup = |name: &str| {
        aliases.get(name).with_context(|| {
            format!(
                "Server alias '{name}' is missing from {}. Specify a server URL or add the alias.",
                config.display()
            )
        })
    };
    let mut value = lookup(host_or_alias)?;
    // A default may select an existing alias or contain a server URL directly.
    if host_or_alias == "default" && !value.contains("://") {
        validate_id(value).context("Invalid default server alias")?;
        value = lookup(value)?;
    }
    origin(value)
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

/// Reads the deployment credential for a server from the operating system's credential store.
pub fn upload_token(server: &str) -> Result<String> {
    let server = origin(server)?;
    read_keyring(&server)?.with_context(|| {
        format!("Sign in to deploy to {server}. Run: parlando login --server {server}")
    })
}

/// Opens the host's dashboard in the default browser. Only the terminal receives the
/// persistent credential, which is saved under the normalized server URL after approval.
pub async fn login(server: &str, open_browser: bool) -> Result<()> {
    #[derive(serde::Deserialize)]
    struct Pairing {
        device: String,
        code: String,
        url: String,
    }
    #[derive(serde::Deserialize)]
    struct Exchange {
        token: Option<String>,
    }
    let server = origin(server)?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    let pairing: Pairing =
        super::management_response(client.post(format!("{server}/api/hosting/cli/start"))).await?;
    let mut url = reqwest::Url::parse(&pairing.url)?;
    origin(&url.origin().ascii_serialization())?;
    ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.path() == "/admin"
            && url.query() == Some("cli-login")
            && url.fragment().is_none(),
        "Invalid browser sign-in URL"
    );
    // The fragment pairs the approval page with this request without entering HTTP logs.
    url.set_fragment(Some(&pairing.code));
    eprintln!("Authorize this CLI in your browser: {url}");
    if open_browser {
        #[cfg(target_os = "macos")]
        let opener = "open";
        #[cfg(not(target_os = "macos"))]
        let opener = "xdg-open";
        if let Ok(mut child) = std::process::Command::new(opener).arg(url.as_str()).spawn() {
            std::thread::spawn(move || child.wait());
        }
    }
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(300);
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let result: Exchange = super::management_response(
            client
                .post(format!("{server}/api/hosting/cli/exchange"))
                .json(&serde_json::json!({"device": pairing.device})),
        )
        .await?;
        if let Some(token) = result.token {
            validate_token(&token)?;
            return write_keyring(&server, &token);
        }
    }
    anyhow::bail!("Browser sign-in expired. Run parlando login again.")
}

/// Reads this account's generic password using the native macOS Keychain API.
#[cfg(target_os = "macos")]
fn read_keyring(account: &str) -> Result<Option<String>> {
    // Security.framework reports an absent generic password as errSecItemNotFound.
    const ITEM_NOT_FOUND: i32 = -25300;
    match security_framework::passwords::get_generic_password(SERVICE, account) {
        Ok(bytes) => Ok(Some(
            String::from_utf8(bytes).context("The saved upload token contains invalid text")?,
        )),
        Err(error) if error.code() == ITEM_NOT_FOUND => Ok(None),
        Err(error) => Err(error).context("Could not read the Parlando upload token from Keychain"),
    }
}

/// Updates the account in the native macOS Keychain without putting secrets in argv.
#[cfg(target_os = "macos")]
fn write_keyring(account: &str, token: &str) -> Result<()> {
    security_framework::passwords::set_generic_password(SERVICE, account, token.as_bytes())
        .context("Could not save the Parlando upload token in Keychain")
}

/// Queries the desktop Secret Service using the libsecret CLI.
#[cfg(target_os = "linux")]
fn read_keyring(account: &str) -> Result<Option<String>> {
    let output = std::process::Command::new("secret-tool")
        .args(["lookup", "service", SERVICE, "account", account])
        .stdin(std::process::Stdio::null())
        .output()
        .context("Could not run secret-tool to read the upload token")?;
    if output.status.code() == Some(1) && output.stderr.is_empty() {
        return Ok(None);
    }
    ensure!(
        output.status.success(),
        "Could not read the Parlando upload token from Secret Service"
    );
    let token =
        String::from_utf8(output.stdout).context("The saved upload token contains invalid text")?;
    Ok(Some(token.trim_end_matches(['\r', '\n']).to_owned()))
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

/// Explains the dashboard upload path when local credential storage is unavailable.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn read_keyring(_: &str) -> Result<Option<String>> {
    anyhow::bail!("CLI credential storage requires macOS Keychain or Linux Secret Service. Use the dashboard to upload on this platform.")
}

/// Reports that this platform cannot persist a browser-authorized CLI credential.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn write_keyring(_: &str, _: &str) -> Result<()> {
    anyhow::bail!("CLI credential storage requires macOS Keychain or Linux Secret Service. Use the dashboard to upload on this platform.")
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
