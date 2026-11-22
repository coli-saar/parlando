//! Durable identities, browser sessions, and audit records for a deployment host.

use std::{fmt, path::Path, sync::Arc, time::Duration};

use anyhow::{anyhow, Result};
use argon2::{
    password_hash::{PasswordHash, PasswordHasher, SaltString},
    Argon2, PasswordVerifier,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::{rngs::OsRng, RngCore};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
    SqlitePool,
};
use tokio::{sync::Semaphore, task};

pub(super) const SESSION_DURATION_SECONDS: i64 = 12 * 60 * 60;
const PASSWORD_SLOTS: usize = 2;
const RECENT_AUDIT_LIMIT: i64 = 200;
const USERNAME_LIMIT: usize = 128;
const PASSWORD_MINIMUM: usize = 12;
const PASSWORD_LIMIT: usize = 1024;

type AccountRow = (i64, String, String, bool);

/// The public fields of a durable dashboard account.
#[derive(Clone, Serialize)]
pub(super) struct Account {
    pub id: i64,
    pub username: String,
    pub role: String,
    pub disabled: bool,
}

impl From<AccountRow> for Account {
    /// Converts a database row that excludes credentials into an account view.
    fn from((id, username, role, disabled): AccountRow) -> Self {
        Self {
            id,
            username,
            role,
            disabled,
        }
    }
}

/// The identity and CSRF value associated with a validated browser session.
pub(super) struct DashboardSession {
    pub account: Account,
    pub csrf: String,
}

/// The cookie secret and authenticated principal returned at sign-in.
pub(super) struct Login {
    pub token: String,
    pub session: DashboardSession,
}

/// A durable record of an action made against the host.
#[derive(Serialize)]
pub(super) struct AuditEvent {
    pub actor: String,
    pub operation: String,
    pub target: String,
    pub outcome: String,
    pub created_at: i64,
}

/// A caller-supplied account value that cannot be stored.
#[derive(Debug)]
pub(super) struct AccountInputError(&'static str);

impl fmt::Display for AccountInputError {
    /// Displays a validation message suitable for the dashboard.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for AccountInputError {}

/// Indicates that every password work slot is occupied.
#[derive(Debug)]
pub(super) struct PasswordWorkBusy;

impl fmt::Display for PasswordWorkBusy {
    /// Displays the transient condition without identifying any account.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Password verification is busy. Try again shortly.")
    }
}

impl std::error::Error for PasswordWorkBusy {}

/// Owns deployment-wide dashboard accounts and their sessions in a dedicated SQLite file.
pub(super) struct AccountStore {
    pool: SqlitePool,
    password_slots: Arc<Semaphore>,
    dummy_hash: String,
}

impl AccountStore {
    /// Opens or initializes the dashboard database under the host's private storage directory.
    pub async fn open(root: &Path) -> Result<Self> {
        let path = root.join("dashboard.sqlite3");
        let mut file = std::fs::OpenOptions::new();
        file.create(true).write(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            file.mode(0o600);
        }
        file.open(&path)?;
        let options = SqliteConnectOptions::new()
            .filename(path)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5))
            .journal_mode(SqliteJournalMode::Wal);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        let mut transaction = pool.begin().await?;
        for statement in [
            "CREATE TABLE IF NOT EXISTS dashboard_account (
                id INTEGER PRIMARY KEY,
                username TEXT NOT NULL UNIQUE,
                role TEXT NOT NULL CHECK (role IN ('operator', 'experimenter')),
                password_hash TEXT NOT NULL,
                disabled INTEGER NOT NULL DEFAULT 0 CHECK (disabled IN (0, 1))
            )",
            "CREATE TABLE IF NOT EXISTS dashboard_session (
                token_digest BLOB PRIMARY KEY NOT NULL,
                account_id INTEGER NOT NULL REFERENCES dashboard_account(id),
                csrf TEXT NOT NULL,
                expires_at INTEGER NOT NULL
            )",
            "CREATE INDEX IF NOT EXISTS dashboard_session_account
                ON dashboard_session (account_id)",
            "CREATE INDEX IF NOT EXISTS dashboard_session_expiration
                ON dashboard_session (expires_at)",
            "CREATE TABLE IF NOT EXISTS dashboard_audit (
                id INTEGER PRIMARY KEY,
                actor TEXT NOT NULL,
                operation TEXT NOT NULL,
                target TEXT NOT NULL,
                outcome TEXT NOT NULL,
                created_at INTEGER NOT NULL
            )",
        ] {
            sqlx::query(statement).execute(&mut *transaction).await?;
        }
        transaction.commit().await?;

        let password_slots = Arc::new(Semaphore::new(PASSWORD_SLOTS));
        let dummy_hash =
            task::spawn_blocking(|| hash_password(random_token().as_bytes())).await??;
        Ok(Self {
            pool,
            password_slots,
            dummy_hash,
        })
    }

    /// Creates a named account with an operator-supplied password and a host-level role.
    pub async fn create(&self, username: &str, password: String, role: &str) -> Result<Account> {
        let username = validate_username(username)?;
        if !matches!(role, "operator" | "experimenter") {
            return Err(AccountInputError("Choose the operator or experimenter role.").into());
        }
        let hash = self.prepare_password(password).await?;
        let row: AccountRow = sqlx::query_as(
            "INSERT INTO dashboard_account (username, role, password_hash) VALUES (?, ?, ?)
             ON CONFLICT(username) DO NOTHING RETURNING id, username, role, disabled",
        )
        .bind(username)
        .bind(role)
        .bind(hash)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(AccountInputError("That username is already in use."))?;
        Ok(Account::from(row))
    }

    /// Checks whether an account has ever been created, including disabled accounts.
    pub async fn is_empty(&self) -> Result<bool> {
        Ok(
            sqlx::query_scalar("SELECT NOT EXISTS (SELECT 1 FROM dashboard_account)")
                .fetch_one(&self.pool)
                .await?,
        )
    }

    /// Creates the first operator only while the account table is empty.
    /// The INSERT tests the condition again in the same statement to serialize concurrent requests.
    pub async fn create_first_operator(
        &self,
        username: &str,
        password: String,
    ) -> Result<Option<Account>> {
        if !self.is_empty().await? {
            return Ok(None);
        }
        let username = validate_username(username)?;
        let hash = self.prepare_password(password).await?;
        let row: Option<AccountRow> = sqlx::query_as(
            "INSERT INTO dashboard_account (username, role, password_hash)
             SELECT ?, 'operator', ? WHERE NOT EXISTS (SELECT 1 FROM dashboard_account)
             RETURNING id, username, role, disabled",
        )
        .bind(username)
        .bind(hash)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(Account::from))
    }

    /// Lists account identities and disabled states for operator review.
    pub async fn list(&self) -> Result<Vec<Account>> {
        let rows: Vec<AccountRow> = sqlx::query_as(
            "SELECT id, username, role, disabled FROM dashboard_account ORDER BY username",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Account::from).collect())
    }

    /// Changes an existing account's password and revokes every active session atomically.
    pub async fn reset_password(&self, id: i64, password: String) -> Result<bool> {
        let hash = self.prepare_password(password).await?;
        self.revoke_account_sessions(
            sqlx::query("UPDATE dashboard_account SET password_hash = ? WHERE id = ?")
                .bind(hash)
                .bind(id),
            id,
        )
        .await
    }

    /// Permanently disables an account and revokes every active session atomically.
    pub async fn disable(&self, id: i64) -> Result<bool> {
        self.revoke_account_sessions(
            sqlx::query("UPDATE dashboard_account SET disabled = 1 WHERE id = ?").bind(id),
            id,
        )
        .await
    }

    /// Applies an account update and removes its sessions in the same transaction.
    async fn revoke_account_sessions<'query>(
        &self,
        update: sqlx::query::Query<'query, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'query>>,
        id: i64,
    ) -> Result<bool> {
        let mut transaction = self.pool.begin().await?;
        let updated = update.execute(&mut *transaction).await?.rows_affected() == 1;
        if updated {
            sqlx::query("DELETE FROM dashboard_session WHERE account_id = ?")
                .bind(id)
                .execute(&mut *transaction)
                .await?;
        }
        transaction.commit().await?;
        Ok(updated)
    }

    /// Enforces the password policy and hashes the value off the asynchronous executor.
    async fn prepare_password(&self, password: String) -> Result<String> {
        if !(PASSWORD_MINIMUM..=PASSWORD_LIMIT).contains(&password.len()) {
            return Err(AccountInputError("Enter a password of 12 to 1024 bytes.").into());
        }
        let slot = self
            .password_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| PasswordWorkBusy)?;
        task::spawn_blocking(move || {
            let _slot = slot;
            hash_password(password.as_bytes())
        })
        .await?
    }

    /// Verifies credentials and issues a durable session if the same credential is still active.
    pub async fn login(&self, username: &str, password: String) -> Result<Option<Login>> {
        if username.len() > USERNAME_LIMIT || password.len() > PASSWORD_LIMIT {
            return Ok(None);
        }
        let slot = self
            .password_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| PasswordWorkBusy)?;
        let row: Option<(i64, String, String, String)> = sqlx::query_as(
            "SELECT id, username, role, password_hash FROM dashboard_account
             WHERE username = ? AND disabled = 0",
        )
        .bind(username.trim())
        .fetch_optional(&self.pool)
        .await?;
        let hash = row.as_ref().map_or(&self.dummy_hash, |row| &row.3).clone();
        let valid = task::spawn_blocking(move || {
            let _slot = slot;
            let parsed = PasswordHash::new(&hash)
                .map_err(|_| anyhow!("stored dashboard password hash is invalid"))?;
            Ok::<_, anyhow::Error>(
                Argon2::default()
                    .verify_password(password.as_bytes(), &parsed)
                    .is_ok(),
            )
        })
        .await??;
        let Some((id, username, role, hash)) = row.filter(|_| valid) else {
            return Ok(None);
        };

        let now = chrono::Utc::now().timestamp();
        let token = random_token();
        let csrf = random_token();
        let mut transaction = self.pool.begin().await?;
        sqlx::query("DELETE FROM dashboard_session WHERE expires_at <= ?")
            .bind(now)
            .execute(&mut *transaction)
            .await?;
        let inserted = sqlx::query(
            "INSERT INTO dashboard_session (token_digest, account_id, csrf, expires_at)
             SELECT ?, id, ?, ? FROM dashboard_account
             WHERE id = ? AND disabled = 0 AND password_hash = ?",
        )
        .bind(token_digest(&token))
        .bind(&csrf)
        .bind(now + SESSION_DURATION_SECONDS)
        .bind(id)
        .bind(hash)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok((inserted.rows_affected() == 1).then_some(Login {
            token,
            session: DashboardSession {
                account: Account {
                    id,
                    username,
                    role,
                    disabled: false,
                },
                csrf,
            },
        }))
    }

    /// Resolves an unexpired cookie to an active account and its CSRF value.
    pub async fn session(&self, token: &str) -> Result<Option<DashboardSession>> {
        let row: Option<(i64, String, String, String)> = sqlx::query_as(
            "SELECT account.id, account.username, account.role, session.csrf
             FROM dashboard_session AS session
             JOIN dashboard_account AS account ON account.id = session.account_id
             WHERE session.token_digest = ? AND session.expires_at > ? AND account.disabled = 0",
        )
        .bind(token_digest(token))
        .bind(chrono::Utc::now().timestamp())
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(id, username, role, csrf)| DashboardSession {
            account: Account {
                id,
                username,
                role,
                disabled: false,
            },
            csrf,
        }))
    }

    /// Revokes a browser session without affecting the account's other sessions.
    pub async fn logout(&self, token: &str) -> Result<()> {
        sqlx::query("DELETE FROM dashboard_session WHERE token_digest = ?")
            .bind(token_digest(token))
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Appends an action record containing validated identifiers and a fixed outcome label.
    pub async fn audit(
        &self,
        actor: &str,
        operation: &str,
        target: &str,
        outcome: &str,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO dashboard_audit (actor, operation, target, outcome, created_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(actor)
        .bind(operation)
        .bind(target)
        .bind(outcome)
        .bind(chrono::Utc::now().timestamp())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Returns the latest audit records, ordered from most recent to oldest.
    pub async fn events(&self) -> Result<Vec<AuditEvent>> {
        let rows = sqlx::query_as::<_, (String, String, String, String, i64)>(
            "SELECT actor, operation, target, outcome, created_at
             FROM dashboard_audit ORDER BY id DESC LIMIT ?",
        )
        .bind(RECENT_AUDIT_LIMIT)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(
                |(actor, operation, target, outcome, created_at)| AuditEvent {
                    actor,
                    operation,
                    target,
                    outcome,
                    created_at,
                },
            )
            .collect())
    }
}

/// Normalizes names and applies the same length and control-character rules to all accounts.
fn validate_username(username: &str) -> Result<&str> {
    let username = username.trim();
    if username.is_empty()
        || username.len() > USERNAME_LIMIT
        || username.chars().any(char::is_control)
    {
        return Err(AccountInputError("Enter a username of 1 to 128 bytes.").into());
    }
    Ok(username)
}

/// Produces a PHC-encoded Argon2id verifier with a fresh random salt.
fn hash_password(password: &[u8]) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password, &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| anyhow!("could not hash dashboard password"))
}

/// Generates an opaque 256-bit value suitable for HTTP cookies and headers.
fn random_token() -> String {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Produces the deterministic lookup digest of an opaque bearer value.
fn token_digest(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Checks that parallel setup writes create only one durable operator across independent pools.
    #[tokio::test]
    async fn initial_operator_is_created_once() {
        let directory = tempfile::tempdir().unwrap();
        let first = AccountStore::open(directory.path()).await.unwrap();
        let second = AccountStore::open(directory.path()).await.unwrap();
        let password = "first-operator-password";
        assert!(first.is_empty().await.unwrap());
        let (left, right) = tokio::join!(
            first.create_first_operator("left", password.into()),
            second.create_first_operator("right", password.into())
        );
        let left = left.unwrap();
        let right = right.unwrap();
        assert_ne!(left.is_some(), right.is_some());
        let account = left.or(right).unwrap();
        assert_eq!(account.role, "operator");
        assert!(!second.is_empty().await.unwrap());
        assert_eq!(second.list().await.unwrap().len(), 1);
        assert!(second
            .login(&account.username, password.into())
            .await
            .unwrap()
            .is_some());
        assert!(first.disable(account.id).await.unwrap());
        assert!(second
            .create_first_operator("later", password.into())
            .await
            .unwrap()
            .is_none());
    }

    /// Checks that session identity survives a restart and follows credential revocations.
    #[tokio::test]
    async fn password_changes_and_disabling_revoke_durable_sessions() {
        let directory = tempfile::tempdir().unwrap();
        let password = "initial-password";
        let replacement = "replacement-password";
        let store = AccountStore::open(directory.path()).await.unwrap();
        let account = store
            .create("adelaide", password.into(), "experimenter")
            .await
            .unwrap();
        let login = store
            .login("adelaide", password.into())
            .await
            .unwrap()
            .unwrap();
        let other_login = store
            .login("adelaide", password.into())
            .await
            .unwrap()
            .unwrap();
        assert_ne!(login.token, other_login.token);
        assert_ne!(login.session.csrf, other_login.session.csrf);

        let password_hash: String =
            sqlx::query_scalar("SELECT password_hash FROM dashboard_account")
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert!(password_hash.starts_with("$argon2id$"));
        let stored_token: Vec<u8> =
            sqlx::query_scalar("SELECT token_digest FROM dashboard_session LIMIT 1")
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert_eq!(stored_token.len(), 32);
        assert_ne!(stored_token, login.token.as_bytes());

        store.logout(&other_login.token).await.unwrap();
        assert!(store.session(&other_login.token).await.unwrap().is_none());
        store.pool.close().await;
        let store = AccountStore::open(directory.path()).await.unwrap();
        let session = store.session(&login.token).await.unwrap().unwrap();
        assert_eq!(session.account.id, account.id);
        assert_eq!(session.account.role, "experimenter");
        assert_eq!(session.csrf, login.session.csrf);

        assert!(store
            .reset_password(account.id, replacement.into())
            .await
            .unwrap());
        assert!(store.session(&login.token).await.unwrap().is_none());
        assert!(store
            .login("adelaide", password.into())
            .await
            .unwrap()
            .is_none());
        let login = store
            .login("adelaide", replacement.into())
            .await
            .unwrap()
            .unwrap();
        assert!(store.disable(account.id).await.unwrap());
        assert!(store.session(&login.token).await.unwrap().is_none());
        assert!(store
            .login("adelaide", replacement.into())
            .await
            .unwrap()
            .is_none());
        assert!(store.list().await.unwrap()[0].disabled);
    }

    /// Checks absolute expiration and retained action metadata in the database.
    #[tokio::test]
    async fn expired_sessions_fail_and_audit_preserves_action_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let store = AccountStore::open(directory.path()).await.unwrap();
        let account = store
            .create("admin", "password-of-the-operator".into(), "operator")
            .await
            .unwrap();
        let session = store
            .login("admin", "password-of-the-operator".into())
            .await
            .unwrap()
            .unwrap();
        let actor = format!("account:{}", account.id);
        store
            .audit(&actor, "select_revision", "experiment-2", "success")
            .await
            .unwrap();
        sqlx::query("UPDATE dashboard_session SET expires_at = 0")
            .execute(&store.pool)
            .await
            .unwrap();
        assert!(store.session(&session.token).await.unwrap().is_none());
        let events = store.events().await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].actor, actor);
        assert_eq!(events[0].operation, "select_revision");
        assert_eq!(events[0].target, "experiment-2");
        assert_eq!(events[0].outcome, "success");
        assert!(events[0].created_at > 0);
    }
}
