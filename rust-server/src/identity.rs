//! Admission and storage use prefixed UUIDs for internal identities. Session construction
//! requests a short public room code and checks for collisions among live rooms.
//! Authentication separately establishes which participant may enter a room.

use uuid::Uuid;

pub fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", Uuid::new_v4().simple())
}

pub fn session_code() -> String {
    Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(6)
        .collect::<String>()
        .to_uppercase()
}
