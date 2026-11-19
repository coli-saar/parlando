//! Durable administrator credentials and authenticated sessions.

use super::{now_iso, SqliteExperimentStore};
use anyhow::Result;

/// Durable administrator credential material loaded by the authentication layer.
///
/// `password_hash` is an Argon2id PHC string. The cleartext password is never
/// accepted by or written to the storage layer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredAdminCredential {
    /// Login name selected during setup.
    pub username: String,
    /// Argon2id PHC password hash; never the cleartext password.
    pub password_hash: String,
    /// Stable authorization role loaded after successful authentication.
    pub role: String,
}

/// Durable server-side administrator session identified by a hash of its browser token.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredAdminSession {
    /// Unkeyed SHA-256 digest of a 256-bit random token; the bearer token is never stored.
    pub token_digest: String,
    /// Stable authorization role captured when the session was issued.
    pub role: String,
    /// Random synchronizer token required on state-changing requests.
    pub csrf_token: String,
    /// Unix timestamp at which the administrator authenticated.
    pub created_at: i64,
    /// Unix timestamp of the latest periodically persisted authenticated use.
    pub last_seen_at: i64,
}

impl SqliteExperimentStore {
    /// Loads the installation administrator credential, if configured.
    pub(crate) async fn admin_credential(&self) -> Result<Option<StoredAdminCredential>> {
        Ok(sqlx::query_as::<_, (String, String, String)>(
            "select username, password_hash, role from administrator_credential where singleton = 1",
        )
        .fetch_optional(&self.pool)
        .await?
        .map(|(username, password_hash, role)| StoredAdminCredential {
            username,
            password_hash,
            role,
        }))
    }

    /// Stores the installation administrator credential.
    pub(crate) async fn create_admin_credential(
        &self,
        credential: StoredAdminCredential,
    ) -> Result<bool> {
        let result = sqlx::query(
            r#"
            insert into administrator_credential
                (singleton, username, password_hash, role, created_at)
            values (1, ?, ?, ?, ?)
            on conflict(singleton) do nothing
            "#,
        )
        .bind(credential.username)
        .bind(credential.password_hash)
        .bind(credential.role)
        .bind(now_iso())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Persists an authenticated administrator session.
    pub(crate) async fn save_admin_session(&self, session: StoredAdminSession) -> Result<()> {
        sqlx::query(
            r#"
            insert into administrator_sessions
                (token_digest, role, csrf_token, created_at, last_seen_at)
            values (?, ?, ?, ?, ?)
            "#,
        )
        .bind(session.token_digest)
        .bind(session.role)
        .bind(session.csrf_token)
        .bind(session.created_at)
        .bind(session.last_seen_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Loads an administrator session by its token digest.
    pub(crate) async fn admin_session(
        &self,
        token_digest: &str,
    ) -> Result<Option<StoredAdminSession>> {
        Ok(sqlx::query_as::<_, (String, String, String, i64, i64)>(
            r#"
            select token_digest, role, csrf_token, created_at, last_seen_at
            from administrator_sessions
            where token_digest = ?
            "#,
        )
        .bind(token_digest)
        .fetch_optional(&self.pool)
        .await?
        .map(
            |(token_digest, role, csrf_token, created_at, last_seen_at)| StoredAdminSession {
                token_digest,
                role,
                csrf_token,
                created_at,
                last_seen_at,
            },
        ))
    }

    /// Records the latest use of an administrator session.
    pub(crate) async fn touch_admin_session(
        &self,
        token_digest: &str,
        last_seen_at: i64,
    ) -> Result<()> {
        sqlx::query("update administrator_sessions set last_seen_at = ? where token_digest = ?")
            .bind(last_seen_at)
            .bind(token_digest)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Revokes one administrator session.
    pub(crate) async fn delete_admin_session(&self, token_digest: &str) -> Result<()> {
        sqlx::query("delete from administrator_sessions where token_digest = ?")
            .bind(token_digest)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Removes administrator sessions older than the supplied cutoffs.
    pub(crate) async fn delete_expired_admin_sessions(
        &self,
        idle_before: i64,
        absolute_before: i64,
    ) -> Result<()> {
        sqlx::query(
            "delete from administrator_sessions where last_seen_at <= ? or created_at <= ?",
        )
        .bind(idle_before)
        .bind(absolute_before)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
