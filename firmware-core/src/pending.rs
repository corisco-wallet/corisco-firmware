use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const MAX_PENDING: usize = 64;
pub const PENDING_TTL: Duration = Duration::from_secs(60);

/// Bounded, expiring, take-once store: an unused FROST nonce must not linger or accumulate.
pub struct PendingStore<T> {
    entries: HashMap<u32, (Instant, T)>,
}

impl<T> PendingStore<T> {
    pub fn new() -> Self {
        Self { entries: HashMap::new() }
    }

    pub fn insert(&mut self, id: u32, value: T) {
        self.insert_at(Instant::now(), id, value);
    }

    pub fn take(&mut self, id: u32) -> Option<T> {
        self.take_at(Instant::now(), id)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    fn insert_at(&mut self, now: Instant, id: u32, value: T) {
        self.entries.retain(|_, (created, _)| now.duration_since(*created) < PENDING_TTL);
        if self.entries.len() >= MAX_PENDING {
            if let Some(oldest) = self.entries.iter().min_by_key(|(_, (created, _))| *created).map(|(id, _)| *id) {
                self.entries.remove(&oldest);
            }
        }
        self.entries.insert(id, (now, value));
    }

    fn take_at(&mut self, now: Instant, id: u32) -> Option<T> {
        let (created, value) = self.entries.remove(&id)?;
        (now.duration_since(created) < PENDING_TTL).then_some(value)
    }
}

impl<T> Default for PendingStore<T> {
    fn default() -> Self {
        Self::new()
    }
}

pub const CLAIM_AUTH_TTL: Duration = Duration::from_secs(600);
pub const MAX_AUTHORIZED_LEAVES: usize = 64;

/// Leaves the device itself saw being claimed; only these may be signed without a tap.
#[derive(Default)]
pub struct AuthorizedLeaves {
    entries: HashMap<String, Instant>,
}

impl AuthorizedLeaves {
    pub fn authorize(&mut self, leaf_id: String) {
        self.authorize_at(Instant::now(), leaf_id);
    }

    pub fn is_authorized(&self, leaf_id: &str) -> bool {
        self.is_authorized_at(Instant::now(), leaf_id)
    }

    fn authorize_at(&mut self, now: Instant, leaf_id: String) {
        self.entries.retain(|_, granted| now.duration_since(*granted) < CLAIM_AUTH_TTL);
        if self.entries.len() >= MAX_AUTHORIZED_LEAVES {
            if let Some(oldest) = self.entries.iter().min_by_key(|(_, granted)| **granted).map(|(id, _)| id.clone()) {
                self.entries.remove(&oldest);
            }
        }
        self.entries.insert(leaf_id, now);
    }

    fn is_authorized_at(&self, now: Instant, leaf_id: &str) -> bool {
        self.entries.get(leaf_id).is_some_and(|granted| now.duration_since(*granted) < CLAIM_AUTH_TTL)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_is_single_use() {
        let mut s = PendingStore::new();
        s.insert(1, "n");
        assert_eq!(s.take(1), Some("n"));
        assert_eq!(s.take(1), None);
    }

    #[test]
    fn expired_entry_is_not_returned() {
        let mut s = PendingStore::new();
        let t0 = Instant::now();
        s.insert_at(t0, 1, "n");
        assert_eq!(s.take_at(t0 + PENDING_TTL, 1), None);
    }

    #[test]
    fn cap_evicts_oldest() {
        let mut s = PendingStore::new();
        let t0 = Instant::now();
        for i in 0..=MAX_PENDING as u32 {
            s.insert_at(t0 + Duration::from_millis(i as u64), i, i);
        }
        assert_eq!(s.take_at(t0, 0), None);
        assert_eq!(s.take_at(t0, MAX_PENDING as u32), Some(MAX_PENDING as u32));
    }

    #[test]
    fn clear_drops_everything() {
        let mut s = PendingStore::new();
        s.insert(1, "n");
        s.clear();
        assert_eq!(s.take(1), None);
    }

    #[test]
    fn authorization_is_per_leaf_and_expires() {
        let mut a = AuthorizedLeaves::default();
        let t0 = Instant::now();
        a.authorize_at(t0, "a".into());
        assert!(a.is_authorized_at(t0, "a"));
        assert!(!a.is_authorized_at(t0, "b"));
        assert!(!a.is_authorized_at(t0 + CLAIM_AUTH_TTL, "a"));
    }

    #[test]
    fn authorization_cap_evicts_oldest() {
        let mut a = AuthorizedLeaves::default();
        let t0 = Instant::now();
        for i in 0..=MAX_AUTHORIZED_LEAVES {
            a.authorize_at(t0 + Duration::from_millis(i as u64), i.to_string());
        }
        assert!(!a.is_authorized_at(t0, "0"));
        assert!(a.is_authorized_at(t0, &MAX_AUTHORIZED_LEAVES.to_string()));
    }
}
