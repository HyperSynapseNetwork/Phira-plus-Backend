//! JoinIntent (design §14.6, contract §8/§21): user confirms "join room" → PPB
//! creates a short-lived intent → listens for PMP `user.online` → matches the
//! phira_id → `room.force_move`. Status polled via `GET /me/join-intents/{id}`:
//! `pending | user_online | moving | completed | failed | expired`.
//!
//! In-memory only (short-lived, restart-safe to lose); no persistence.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::Serialize;
use uuid::Uuid;

use crate::error::ApiError;

/// Default intent TTL (seconds).
pub const DEFAULT_TTL_SECS: i64 = 300;
/// Maximum allowed TTL (seconds).
pub const MAX_TTL_SECS: i64 = 900;
/// How long terminal/expired intents are retained for status polling.
pub const TERMINAL_RETENTION_SECS: i64 = 3600;

pub const STATUS_PENDING: &str = "pending";
pub const STATUS_USER_ONLINE: &str = "user_online";
pub const STATUS_MOVING: &str = "moving";
pub const STATUS_COMPLETED: &str = "completed";
pub const STATUS_FAILED: &str = "failed";
pub const STATUS_EXPIRED: &str = "expired";

#[derive(Debug, Clone, Serialize)]
pub struct JoinIntent {
    pub id: Uuid,
    pub user_id: Uuid,
    pub phira_id: i64,
    pub room_id: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub status: String,
}

fn is_terminal(status: &str) -> bool {
    matches!(status, STATUS_COMPLETED | STATUS_FAILED | STATUS_EXPIRED)
}

/// The status a client should see (an expired non-terminal intent reads as
/// `expired`).
fn effective_status(intent: &JoinIntent) -> String {
    if intent.expires_at <= Utc::now() && !is_terminal(&intent.status) {
        STATUS_EXPIRED.to_string()
    } else {
        intent.status.clone()
    }
}

/// In-memory intent store. Keyed by PPB user uuid; indexed by phira_id for the
/// `user.online` matcher.
#[derive(Clone, Default)]
pub struct JoinIntentStore {
    intents: Arc<DashMap<Uuid, JoinIntent>>,
    by_phira: Arc<DashMap<i64, Uuid>>,
}

impl JoinIntentStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create(
        &self,
        user_id: Uuid,
        phira_id: i64,
        room_id: &str,
        ttl_secs: Option<i64>,
    ) -> Result<JoinIntent, ApiError> {
        let ttl = ttl_secs.unwrap_or(DEFAULT_TTL_SECS);
        if !(1..=MAX_TTL_SECS).contains(&ttl) {
            return Err(ApiError::validation(format!(
                "ttl_secs must be between 1 and {MAX_TTL_SECS}"
            )));
        }
        if room_id.trim().is_empty() {
            return Err(ApiError::validation("room_id required"));
        }
        let now = Utc::now();
        let intent = JoinIntent {
            id: Uuid::new_v4(),
            user_id,
            phira_id,
            room_id: room_id.trim().to_string(),
            created_at: now,
            expires_at: now + chrono::Duration::seconds(ttl),
            status: STATUS_PENDING.to_string(),
        };
        // Replace any existing intent for this user (one join target at a time).
        if let Some(prev) = self.intents.insert(intent.id, intent.clone()) {
            self.by_phira.remove(&prev.phira_id);
        }
        self.by_phira.insert(phira_id, intent.id);
        Ok(intent)
    }

    /// Fetch an intent by id for `user_id` (owner-only); returns the effective
    /// status (expired when past TTL).
    pub fn get(&self, user_id: Uuid, intent_id: Uuid) -> Result<JoinIntent, ApiError> {
        let entry = self.intents.get(&intent_id).map(|i| i.clone());
        match entry {
            Some(intent) if intent.user_id == user_id => {
                let mut out = intent.clone();
                out.status = effective_status(&intent);
                Ok(out)
            }
            Some(_) => Err(ApiError::permission_denied()),
            None => Err(ApiError::not_found("join intent")),
        }
    }

    pub fn cancel(&self, user_id: Uuid, intent_id: Uuid) -> Result<(), ApiError> {
        // Extract ownership info first, dropping the DashMap shard guard before
        // mutating. Holding a `Ref` while `remove`-ing the same shard deadlocks.
        let entry = self.intents.get(&intent_id).map(|i| (i.user_id, i.phira_id));
        match entry {
            Some((owner, phira_id)) if owner == user_id => {
                self.by_phira.remove(&phira_id);
                self.intents.remove(&intent_id);
                Ok(())
            }
            Some(_) => Err(ApiError::permission_denied()),
            None => Err(ApiError::not_found("join intent")),
        }
    }

    /// List the caller's recent intents (active + retained terminal ones).
    pub fn list_for_user(&self, user_id: Uuid) -> Vec<JoinIntent> {
        let retention = Utc::now() - chrono::Duration::seconds(TERMINAL_RETENTION_SECS);
        self.intents
            .iter()
            .filter(|e| e.user_id == user_id && e.expires_at > retention)
            .map(|e| {
                let mut out = e.clone();
                out.status = effective_status(&e);
                out
            })
            .collect()
    }

    /// Mark a pending intent as seen-online (user_online) and return it, so the
    /// caller can `room.force_move`. Does not remove the intent.
    pub fn match_online(&self, phira_id: i64) -> Option<JoinIntent> {
        let intent_id = *self.by_phira.get(&phira_id)?.value();
        let intent = self.intents.get(&intent_id).map(|i| i.clone())?;
        if effective_status(&intent) != STATUS_PENDING {
            return None;
        }
        self.update_status(&intent_id, STATUS_USER_ONLINE);
        let mut out = intent;
        out.status = STATUS_USER_ONLINE.to_string();
        Some(out)
    }

    pub fn mark_moving(&self, intent_id: &Uuid) {
        self.update_status(intent_id, STATUS_MOVING);
    }

    /// Set a terminal status and drop the phira_id index (a fresh intent may
    /// then be created/matched for the same player).
    pub fn mark_terminal(&self, intent_id: &Uuid, status: &str) {
        let phira_id = self.intents.get(intent_id).map(|i| i.phira_id);
        self.update_status(intent_id, status);
        if let Some(p) = phira_id {
            self.by_phira.remove(&p);
        }
    }

    fn update_status(&self, intent_id: &Uuid, status: &str) {
        if let Some(mut e) = self.intents.get_mut(intent_id) {
            e.status = status.to_string();
        }
    }

    /// Mark expired non-terminal intents, and remove terminal/expired intents
    /// past retention. Returns how many were removed.
    pub fn cleanup_expired(&self) -> usize {
        let now = Utc::now();
        let ids: Vec<Uuid> = self.intents.iter().map(|e| e.id).collect();
        let mut removed = 0;
        for id in ids {
            let (expires_at, status) = self
                .intents
                .get(&id)
                .map(|i| (i.expires_at, i.status.clone()))
                .unwrap_or_default();
            if expires_at <= now {
                if is_terminal(&status) {
                    if now - expires_at > chrono::Duration::seconds(TERMINAL_RETENTION_SECS) {
                        if let Some((_, intent)) = self.intents.remove(&id) {
                            self.by_phira.remove(&intent.phira_id);
                            removed += 1;
                        }
                    }
                } else {
                    self.update_status(&id, STATUS_EXPIRED);
                }
            }
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> JoinIntentStore {
        JoinIntentStore::new()
    }

    #[test]
    fn create_pending_and_get() {
        let s = store();
        let uid = Uuid::new_v4();
        let intent = s.create(uid, 42, "ABC", Some(300)).unwrap();
        assert_eq!(intent.status, STATUS_PENDING);
        let got = s.get(uid, intent.id).unwrap();
        assert_eq!(got.status, STATUS_PENDING);
    }

    #[test]
    fn rejects_bad_ttl() {
        let s = store();
        assert!(s.create(Uuid::new_v4(), 1, "ABC", Some(0)).is_err());
        assert!(s.create(Uuid::new_v4(), 1, "ABC", Some(MAX_TTL_SECS + 1)).is_err());
        assert!(s.create(Uuid::new_v4(), 1, "  ", Some(300)).is_err());
    }

    #[test]
    fn online_then_terminal_flow() {
        let s = store();
        let uid = Uuid::new_v4();
        let intent = s.create(uid, 77, "ROOM1", Some(300)).unwrap();
        let matched = s.match_online(77).unwrap();
        assert_eq!(matched.status, STATUS_USER_ONLINE);
        s.mark_moving(&intent.id);
        assert_eq!(s.get(uid, intent.id).unwrap().status, STATUS_MOVING);
        s.mark_terminal(&intent.id, STATUS_COMPLETED);
        assert_eq!(s.get(uid, intent.id).unwrap().status, STATUS_COMPLETED);
        // Terminal intent no longer matches a fresh user.online.
        assert!(s.match_online(77).is_none());
    }

    #[test]
    fn get_requires_owner() {
        let s = store();
        let uid = Uuid::new_v4();
        let intent = s.create(uid, 9, "R", Some(300)).unwrap();
        assert!(s.get(Uuid::new_v4(), intent.id).is_err());
        assert!(s.cancel(Uuid::new_v4(), intent.id).is_err());
        assert!(s.cancel(uid, intent.id).is_ok());
    }

    #[test]
    fn cancelled_intent_not_matched() {
        let s = store();
        let uid = Uuid::new_v4();
        let intent = s.create(uid, 9, "R", Some(300)).unwrap();
        s.cancel(uid, intent.id).unwrap();
        assert!(s.match_online(9).is_none());
    }

    #[test]
    fn expired_reads_as_expired() {
        let s = store();
        let uid = Uuid::new_v4();
        let stale = JoinIntent {
            id: Uuid::new_v4(),
            user_id: uid,
            phira_id: 6,
            room_id: "R".to_string(),
            created_at: Utc::now() - chrono::Duration::seconds(10),
            expires_at: Utc::now() - chrono::Duration::seconds(1),
            status: STATUS_PENDING.to_string(),
        };
        s.intents.insert(stale.id, stale.clone());
        s.by_phira.insert(6, stale.id);
        assert_eq!(s.get(uid, stale.id).unwrap().status, STATUS_EXPIRED);
        assert!(s.match_online(6).is_none());
    }

    #[test]
    fn cleanup_marks_expired_and_removes_old() {
        let s = store();
        let uid = Uuid::new_v4();
        // Old terminal intent past retention -> removed.
        let old_terminal = JoinIntent {
            id: Uuid::new_v4(),
            user_id: uid,
            phira_id: 6,
            room_id: "R".to_string(),
            created_at: Utc::now() - chrono::Duration::seconds(4000),
            expires_at: Utc::now() - chrono::Duration::seconds(3700),
            status: STATUS_COMPLETED.to_string(),
        };
        s.intents.insert(old_terminal.id, old_terminal.clone());
        s.by_phira.insert(6, old_terminal.id);
        let n = s.cleanup_expired();
        assert_eq!(n, 1);
    }
}
