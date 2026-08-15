//! Authentication primitives for the administrator and participant trust planes.

use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Result};
use argon2::{
    password_hash::{PasswordHash, PasswordHasher, SaltString},
    Argon2, PasswordVerifier,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use rand::{rngs::OsRng, RngCore};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use tokio::{
    sync::{RwLock, Semaphore},
    task,
};

use crate::storage::{SharedExperimentStore, StoredAdminCredential};

const PARTICIPANT_LIFETIME_SECONDS: i64 = 24 * 60 * 60;
const ADMIN_IDLE_SECONDS: i64 = 30 * 60;
const ADMIN_ABSOLUTE_SECONDS: i64 = 8 * 60 * 60;
const TICKET_LIFETIME_SECONDS: i64 = 60;

/// The authenticated identity attached to a participant-owned request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ParticipantPrincipal {
    /// Non-secret participant-session identifier used for internal correlation.
    pub participant_session_id: String,
    /// Credential generation used to revoke outstanding upgrade tickets.
    pub generation: u64,
}

#[derive(Clone, Debug)]
struct ParticipantCredentialRecord {
    participant_session_id: String,
    generation: u64,
    expires_at: i64,
    revoked: bool,
}

/// In-memory participant credential registry for the active server process.
pub(crate) struct ParticipantAuthenticator {
    pepper: [u8; 32],
    records: RwLock<HashMap<[u8; 32], ParticipantCredentialRecord>>,
}

impl Default for ParticipantAuthenticator {
    /// Creates a registry with a fresh process-local hash key.
    fn default() -> Self {
        Self {
            pepper: random_bytes(),
            records: RwLock::new(HashMap::new()),
        }
    }
}

impl ParticipantAuthenticator {
    /// Issues a new high-entropy credential and stores only its SHA-256 digest.
    pub async fn issue(&self, participant_session_id: String) -> String {
        let credential = random_token("par_");
        self.records.write().await.insert(
            token_digest(&credential, &self.pepper),
            ParticipantCredentialRecord {
                participant_session_id,
                generation: 1,
                expires_at: now_timestamp() + PARTICIPANT_LIFETIME_SECONDS,
                revoked: false,
            },
        );
        credential
    }

    /// Resolves an unexpired, unrevoked bearer credential to its participant principal.
    pub async fn authenticate(&self, credential: &str) -> Option<ParticipantPrincipal> {
        let now = now_timestamp();
        let record = self
            .records
            .read()
            .await
            .get(&token_digest(credential, &self.pepper))
            .cloned()?;
        (!record.revoked && record.expires_at > now).then_some(ParticipantPrincipal {
            participant_session_id: record.participant_session_id,
            generation: record.generation,
        })
    }

    /// Checks whether a ticket's participant and credential generation remain active.
    pub async fn generation_is_active(
        &self,
        participant_session_id: &str,
        generation: u64,
    ) -> bool {
        let now = now_timestamp();
        self.records.read().await.values().any(|record| {
            !record.revoked
                && record.expires_at > now
                && record.participant_session_id == participant_session_id
                && record.generation == generation
        })
    }

    /// Removes expired credentials and returns sessions with no remaining active credential.
    pub async fn cleanup(&self) -> HashSet<String> {
        let now = now_timestamp();
        let mut records = self.records.write().await;
        let expired = records
            .values()
            .filter(|record| record.revoked || record.expires_at <= now)
            .map(|record| record.participant_session_id.clone())
            .collect::<HashSet<_>>();
        records.retain(|_, record| !record.revoked && record.expires_at > now);
        let active = records
            .values()
            .map(|record| record.participant_session_id.clone())
            .collect::<HashSet<_>>();
        expired.difference(&active).cloned().collect()
    }

    /// Revokes every credential for one abandoned unattached participant session.
    pub async fn revoke_participant_session(&self, participant_session_id: &str) {
        self.records
            .write()
            .await
            .retain(|_, record| record.participant_session_id != participant_session_id);
    }
}

/// Purpose assigned to a one-use browser WebSocket upgrade ticket.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UpgradePurpose {
    /// A game-state WebSocket upgrade.
    Game,
    /// A PCM audio WebSocket upgrade.
    Audio,
}

/// Authenticated claims recovered after atomically consuming an upgrade ticket.
#[derive(Clone, Debug)]
pub(crate) struct UpgradeTicketClaims {
    /// Room to which the ticket is bound.
    pub room_id: String,
    /// Participant that requested the ticket.
    pub participant_session_id: String,
    /// Authoritative role at ticket issuance.
    pub role: String,
    /// Credential generation at ticket issuance.
    pub generation: u64,
    /// Upgrade purpose, preventing game/audio cross-protocol replay.
    pub purpose: UpgradePurpose,
    pub expires_at: i64,
}

/// One-use, short-lived upgrade-ticket registry.
pub(crate) struct UpgradeTicketStore {
    pepper: [u8; 32],
    tickets: RwLock<HashMap<[u8; 32], UpgradeTicketClaims>>,
}

impl Default for UpgradeTicketStore {
    /// Creates a ticket registry with a fresh process-local hash key.
    fn default() -> Self {
        Self {
            pepper: random_bytes(),
            tickets: RwLock::new(HashMap::new()),
        }
    }
}

impl UpgradeTicketStore {
    /// Mints a random ticket while retaining only its digest.
    pub async fn issue(&self, mut claims: UpgradeTicketClaims) -> String {
        claims.expires_at = now_timestamp() + TICKET_LIFETIME_SECONDS;
        let ticket = random_token("wst_");
        let now = now_timestamp();
        let mut tickets = self.tickets.write().await;
        tickets.retain(|_, existing| existing.expires_at > now);
        tickets.retain(|_, existing| {
            existing.room_id != claims.room_id
                || existing.participant_session_id != claims.participant_session_id
                || existing.purpose != claims.purpose
        });
        tickets.insert(token_digest(&ticket, &self.pepper), claims);
        ticket
    }

    /// Atomically consumes a ticket and verifies its purpose, room, and lifetime.
    pub async fn consume(
        &self,
        ticket: &str,
        purpose: UpgradePurpose,
        room_id: &str,
    ) -> Option<UpgradeTicketClaims> {
        let claims = self
            .tickets
            .write()
            .await
            .remove(&token_digest(ticket, &self.pepper))?;
        (claims.expires_at > now_timestamp()
            && claims.purpose == purpose
            && claims.room_id == room_id)
            .then_some(claims)
    }

    /// Removes expired upgrade tickets.
    pub async fn cleanup(&self) {
        let now = now_timestamp();
        self.tickets
            .write()
            .await
            .retain(|_, claims| claims.expires_at > now);
    }
}

/// Coarse administrator capability assigned at login.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AdminRole {
    /// May monitor sessions and create exports.
    Operator,
    /// May also create experiments and change lifecycle state.
    Administrator,
}

impl AdminRole {
    /// Parses the configured role name, rejecting unknown privilege labels.
    fn parse(value: &str) -> Result<Self> {
        match value {
            "operator" => Ok(Self::Operator),
            "administrator" => Ok(Self::Administrator),
            _ => Err(anyhow!(
                "administrator role must be operator or administrator"
            )),
        }
    }

    /// Returns whether this role may perform administrator-only mutations.
    pub fn may_mutate_experiments(self) -> bool {
        self == Self::Administrator
    }

    /// Returns the stable storage representation of this capability.
    fn as_str(self) -> &'static str {
        match self {
            Self::Operator => "operator",
            Self::Administrator => "administrator",
        }
    }
}

#[derive(Clone)]
struct AdminCredential {
    username: String,
    password_hash: String,
    role: AdminRole,
}

/// Server-side administrator session referenced by the secure cookie.
#[derive(Clone, Debug)]
pub(crate) struct AdminSession {
    /// Session capability role.
    pub role: AdminRole,
    /// Random token required on state-changing requests.
    pub csrf_token: String,
    pub created_at: i64,
    pub last_seen_at: i64,
}

/// Built-in Argon2id administrator authenticator and server-side session registry.
pub(crate) struct AdminAuthenticator {
    credential: RwLock<Option<AdminCredential>>,
    store: SharedExperimentStore,
    sessions: RwLock<HashMap<[u8; 32], AdminSession>>,
    session_pepper: [u8; 32],
    login_slots: Semaphore,
}

/// Result of one bounded administrator password-verification request.
pub(crate) enum AdminLoginResult {
    /// The credential was valid and a server-side session was created.
    Authenticated(String, AdminSession),
    /// The supplied username or password was invalid.
    Invalid,
    /// Both password-verification slots are already occupied.
    Busy,
}

impl AdminAuthenticator {
    /// Loads the database-backed administrator credential, if setup is complete.
    pub async fn load(store: SharedExperimentStore) -> Result<Self> {
        let credential = store
            .admin_credential()
            .await?
            .map(Self::credential_from_stored)
            .transpose()?;
        Ok(Self {
            credential: RwLock::new(credential),
            store,
            sessions: RwLock::new(HashMap::new()),
            session_pepper: random_bytes(),
            login_slots: Semaphore::new(2),
        })
    }

    /// Validates one stored credential before it becomes authentication state.
    fn credential_from_stored(stored: StoredAdminCredential) -> Result<AdminCredential> {
        Self::validated_credential(
            stored.username,
            stored.password_hash,
            AdminRole::parse(&stored.role)?,
        )
    }

    /// Ensures only Argon2id PHC hashes enter the active credential slot.
    fn validated_credential(
        username: String,
        password_hash: String,
        role: AdminRole,
    ) -> Result<AdminCredential> {
        let parsed = PasswordHash::new(&password_hash)
            .map_err(|error| anyhow!("invalid administrator password hash: {error}"))?;
        if parsed.algorithm.as_str() != "argon2id" {
            return Err(anyhow!("administrator password hash must use Argon2id"));
        }
        Ok(AdminCredential {
            username,
            password_hash,
            role,
        })
    }

    /// Returns whether a usable administrator credential was configured.
    pub async fn is_configured(&self) -> bool {
        self.credential.read().await.is_some()
    }

    /// Creates and persists the first administrator, if setup is still open.
    pub async fn setup(&self, username: &str, password: &str) -> Result<bool> {
        let username = username.trim();
        if username.is_empty() || username.len() > 128 {
            return Err(anyhow!(
                "administrator username must contain 1 to 128 characters"
            ));
        }
        if password.len() < 12 || password.len() > 1024 {
            return Err(anyhow!(
                "administrator password must contain 12 to 1024 characters"
            ));
        }
        if self.credential.read().await.is_some() {
            return Ok(false);
        }
        let role = AdminRole::Administrator;
        let salt = SaltString::generate(&mut OsRng);
        let password_hash = Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map_err(|error| anyhow!("failed to hash administrator password: {error}"))?
            .to_string();
        let stored = StoredAdminCredential {
            username: username.to_string(),
            password_hash,
            role: role.as_str().to_string(),
        };
        if !self.store.create_admin_credential(stored.clone()).await? {
            return Ok(false);
        }
        *self.credential.write().await = Some(Self::credential_from_stored(stored)?);
        Ok(true)
    }

    /// Verifies credentials with Argon2 and creates a random server-side session.
    pub async fn login(&self, username: &str, password: &str) -> Result<AdminLoginResult> {
        let Ok(_slot) = self.login_slots.try_acquire() else {
            return Ok(AdminLoginResult::Busy);
        };
        let now = now_timestamp();
        let credential = self.credential.read().await.clone();
        let Some(credential) = credential else {
            return Ok(AdminLoginResult::Invalid);
        };
        let username_valid: bool = token_digest(username, &self.session_pepper)
            .ct_eq(&token_digest(&credential.username, &self.session_pepper))
            .into();
        let password_hash = credential.password_hash.clone();
        let password = password.to_string();
        let password_valid = task::spawn_blocking(move || -> Result<bool> {
            let parsed = PasswordHash::new(&password_hash)
                .map_err(|error| anyhow!("administrator password hash is invalid: {error}"))?;
            Ok(Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok())
        })
        .await
        .map_err(|error| anyhow!("administrator password task failed: {error}"))??;
        let valid = username_valid & password_valid;
        if !valid {
            return Ok(AdminLoginResult::Invalid);
        }
        let token = random_token("adm_");
        let session = AdminSession {
            role: credential.role,
            csrf_token: random_token("csrf_"),
            created_at: now,
            last_seen_at: now,
        };
        self.sessions
            .write()
            .await
            .insert(token_digest(&token, &self.session_pepper), session.clone());
        Ok(AdminLoginResult::Authenticated(token, session))
    }

    /// Validates a session token and refreshes its idle timestamp.
    pub async fn authenticate(&self, token: &str) -> Option<AdminSession> {
        let now = now_timestamp();
        let mut sessions = self.sessions.write().await;
        let digest = token_digest(token, &self.session_pepper);
        let session = sessions.get_mut(&digest)?;
        if session.last_seen_at <= now - ADMIN_IDLE_SECONDS
            || session.created_at <= now - ADMIN_ABSOLUTE_SECONDS
        {
            sessions.remove(&digest);
            return None;
        }
        session.last_seen_at = now;
        Some(session.clone())
    }

    /// Revokes a server-side administrator session.
    pub async fn logout(&self, token: &str) {
        self.sessions
            .write()
            .await
            .remove(&token_digest(token, &self.session_pepper));
    }

    /// Removes expired administrator sessions.
    pub async fn cleanup(&self) {
        let now = now_timestamp();
        self.sessions.write().await.retain(|_, session| {
            session.last_seen_at > now - ADMIN_IDLE_SECONDS
                && session.created_at > now - ADMIN_ABSOLUTE_SECONDS
        });
    }
}

/// Generates a 256-bit opaque token encoded for safe header and URL use.
fn random_token(prefix: &str) -> String {
    let bytes = random_bytes();
    format!("{prefix}{}", URL_SAFE_NO_PAD.encode(bytes))
}

/// Generates 256 bits from the operating-system random source.
fn random_bytes() -> [u8; 32] {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    bytes
}

/// Produces the keyed fixed-width digest retained for an opaque bearer value.
fn token_digest(token: &str, pepper: &[u8; 32]) -> [u8; 32] {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(pepper).expect("SHA-256 accepts 32-byte HMAC keys");
    mac.update(token.as_bytes());
    mac.finalize().into_bytes().into()
}

/// Returns the current UTC timestamp in seconds.
fn now_timestamp() -> i64 {
    chrono::Utc::now().timestamp()
}

#[cfg(test)]
mod admin_setup_tests {
    use super::*;
    use crate::storage::experiment_store_from_url;

    /// Confirms setup hashes the password, closes setup, and enables authentication.
    #[tokio::test]
    async fn first_admin_setup_hashes_password_and_authenticates() {
        let store = experiment_store_from_url("sqlite:///:memory:")
            .await
            .unwrap();
        let auth = AdminAuthenticator::load(store.clone()).await.unwrap();

        assert!(!auth.is_configured().await);
        assert!(auth
            .setup("researcher", "a-long-test-password")
            .await
            .unwrap());
        assert!(auth.is_configured().await);
        assert!(!auth.setup("second", "another-test-password").await.unwrap());
        assert!(matches!(
            auth.login("researcher", "a-long-test-password")
                .await
                .unwrap(),
            AdminLoginResult::Authenticated(_, _)
        ));
        assert!(matches!(
            auth.login("researcher", "wrong-password").await.unwrap(),
            AdminLoginResult::Invalid
        ));

        let stored = store.admin_credential().await.unwrap().unwrap();
        assert_ne!(stored.password_hash, "a-long-test-password");
        assert!(stored.password_hash.starts_with("$argon2id$"));
    }

    /// Confirms the browser setup path enforces the minimum password length server-side.
    #[tokio::test]
    async fn admin_setup_rejects_short_passwords() {
        let store = experiment_store_from_url("sqlite:///:memory:")
            .await
            .unwrap();
        let auth = AdminAuthenticator::load(store).await.unwrap();

        assert!(auth.setup("researcher", "too-short").await.is_err());
        assert!(!auth.is_configured().await);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Confirms participant credentials are opaque, resolvable, and not stored as plaintext keys.
    #[tokio::test]
    async fn participant_credentials_resolve_to_the_issued_principal() {
        let auth = ParticipantAuthenticator::default();
        let credential = auth.issue("participant-public-id".to_string()).await;
        assert!(!auth
            .records
            .read()
            .await
            .keys()
            .any(|key| key.as_slice() == credential.as_bytes()));
        assert_eq!(
            auth.authenticate(&credential)
                .await
                .unwrap()
                .participant_session_id,
            "participant-public-id"
        );
    }

    /// Confirms upgrade tickets cannot be replayed or exchanged across protocols.
    #[tokio::test]
    async fn upgrade_tickets_are_one_use_and_purpose_bound() {
        let store = UpgradeTicketStore::default();
        let ticket = store
            .issue(UpgradeTicketClaims {
                room_id: "room".to_string(),
                participant_session_id: "participant".to_string(),
                role: "A".to_string(),
                generation: 1,
                purpose: UpgradePurpose::Game,
                expires_at: 0,
            })
            .await;
        assert!(store
            .consume(&ticket, UpgradePurpose::Audio, "room")
            .await
            .is_none());
        assert!(store
            .consume(&ticket, UpgradePurpose::Game, "room")
            .await
            .is_none());
    }
}
