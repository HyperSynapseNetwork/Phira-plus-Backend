//! JoinIntent (design §14.6, contract §8): user confirms "join room" → PPB
//! creates a short-lived intent → listens for PMP `user.online` → matches the
//! phira_id → `room.force_move`. Expired/cancelled intents are cleaned up.
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

#[derive(Debug, Clone, Serialize)]
pub struct JoinIntent {
    pub id: Uuid,
    pub user_id: Uuid,
    pub phira_id: i64,
    pub room_id: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
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
        };
        // Replace any existing intent for this user (one join target at a time).
        if let Some(prev) = self.intents.insert(intent.id, intent.clone()) {
            self.by_phira.remove(&prev.phira_id);
        }
        self.by_phira.insert(phira_id, intent.id);
        Ok(intent)
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

    pub fn list_for_user(&self, user_id: Uuid) -> Vec<JoinIntent> {
        let now = Utc::now();
        self.intents
            .iter()
            .filter(|e| e.user_id == user_id && e.expires_at > now)
            .map(|e| e.clone())
            .collect()
    }

    /// Consume an intent for a phira_id that just came online. Removes and
    /// returns it so the caller can `room.force_move`.
    pub fn match_online(&self, phira_id: i64) -> Option<JoinIntent> {
        let intent_id = self.by_phira.remove(&phira_id)?.1;
        let intent = self.intents.remove(&intent_id).map(|(_, v)| v)?;
        if intent.expires_at <= Utc::now() {
            return None;
        }
        Some(intent)
    }

    /// Remove expired intents; returns how many were cleaned.
    pub fn cleanup_expired(&self) -> usize {
        let now = Utc::now();
        let expired: Vec<Uuid> = self
            .intents
            .iter()
            .filter(|e| e.expires_at <= now)
            .map(|e| e.id)
            .collect();
        let mut n = 0;
        for id in expired {
            if let Some((_, intent)) = self.intents.remove(&id) {
                self.by_phira.remove(&intent.phira_id);
                n += 1;
            }
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> JoinIntentStore {
        JoinIntentStore::new()
    }

    #[test]
    fn create_and_list() {
        let s = store();
        let intent = s.create(Uuid::new_v4(), 42, "ABC", Some(300)).unwrap();
        assert_eq!(s.list_for_user(intent.user_id).len(), 1);
    }

    #[test]
    fn rejects_bad_ttl() {
        let s = store();
        assert!(s.create(Uuid::new_v4(), 1, "ABC", Some(0)).is_err());
        assert!(s.create(Uuid::new_v4(), 1, "ABC", Some(MAX_TTL_SECS + 1)).is_err());
        assert!(s.create(Uuid::new_v4(), 1, "  ", Some(300)).is_err());
    }

    #[test]
    fn match_online_consumes_and_force_moves() {
        let s = store();
        let uid = Uuid::new_v4();
        s.create(uid, 77, "ROOM1", Some(300)).unwrap();
        let intent = s.match_online(77).unwrap();
        assert_eq!(intent.room_id, "ROOM1");
        assert!(s.match_online(77).is_none(), "consumed exactly once");
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
    fn cancel_requires_owner() {
        let s = store();
        let uid = Uuid::new_v4();
        let intent = s.create(uid, 9, "R", Some(300)).unwrap();
        assert!(s.cancel(Uuid::new_v4(), intent.id).is_err());
        assert!(s.cancel(uid, intent.id).is_ok());
    }

    #[test]
    fn expired_cleaned() {
        let s = store();
        let uid = Uuid::new_v4();
        // Long-lived intent stays valid; the stale one is the only cleanup target.
        s.create(uid, 5, "R", Some(MAX_TTL_SECS)).unwrap();
        let stale = JoinIntent {
            id: Uuid::new_v4(),
            user_id: uid,
            phira_id: 6,
            room_id: "R".to_string(),
            created_at: Utc::now() - chrono::Duration::seconds(10),
            expires_at: Utc::now() - chrono::Duration::seconds(1),
        };
        s.intents.insert(stale.id, stale.clone());
        s.by_phira.insert(6, stale.id);
        let n = s.cleanup_expired();
        assert_eq!(n, 1);
    }
}
