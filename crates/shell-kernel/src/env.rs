use std::collections::BTreeMap;

use rustc_hash::FxHashMap;

#[derive(Debug, Clone)]
pub struct SessionEnv {
    overlay: FxHashMap<String, Option<String>>,
    base: Option<BTreeMap<String, String>>,
}

impl Default for SessionEnv {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionEnv {
    pub fn new() -> Self {
        Self {
            overlay: FxHashMap::default(),
            base: None,
        }
    }

    pub fn from_map(map: BTreeMap<String, String>) -> Self {
        Self {
            overlay: FxHashMap::default(),
            base: Some(map),
        }
    }

    pub fn get(&self, key: &str) -> Option<String> {
        if let Some(entry) = self.overlay.get(key) {
            return entry.clone();
        }
        if let Some(ref b) = self.base {
            return b.get(key).cloned();
        }
        std::env::var(key).ok()
    }

    pub fn insert(&mut self, key: String, val: String) {
        self.overlay.insert(key, Some(val));
    }

    pub fn remove(&mut self, key: &str) {
        self.overlay.insert(key.to_string(), None);
    }

    pub fn materialize(&self) -> BTreeMap<String, String> {
        let mut out: BTreeMap<String, String> = match &self.base {
            Some(b) => b.clone(),
            None => std::env::vars().collect(),
        };
        for (k, v) in &self.overlay {
            match v {
                Some(val) => {
                    out.insert(k.clone(), val.clone());
                }
                None => {
                    out.remove(k);
                }
            }
        }
        out
    }
}
