//! In-memory admin sessions after username/password login.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tailsvc_common::auth::generate_token;

#[derive(Default)]
pub struct SessionStore {
    inner: Mutex<HashMap<String, Instant>>,
    ttl: Duration,
}

impl SessionStore {
    pub fn new(ttl: Duration) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            ttl,
        }
    }

    pub fn create(&self) -> String {
        let token = generate_token(32);
        let exp = Instant::now() + self.ttl;
        if let Ok(mut g) = self.inner.lock() {
            Self::purge_locked(&mut g);
            g.insert(token.clone(), exp);
        }
        token
    }

    pub fn valid(&self, token: &str) -> bool {
        let Ok(mut g) = self.inner.lock() else {
            return false;
        };
        Self::purge_locked(&mut g);
        match g.get(token) {
            Some(exp) if *exp > Instant::now() => true,
            Some(_) => {
                g.remove(token);
                false
            }
            None => false,
        }
    }

    pub fn revoke(&self, token: &str) {
        if let Ok(mut g) = self.inner.lock() {
            g.remove(token);
        }
    }

    fn purge_locked(map: &mut HashMap<String, Instant>) {
        let now = Instant::now();
        map.retain(|_, exp| *exp > now);
    }
}
