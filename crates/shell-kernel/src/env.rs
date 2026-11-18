use std::collections::BTreeMap;

use rustc_hash::FxHashMap;

#[derive(Debug, Clone)]
pub struct SessionEnv {
    overlay: FxHashMap<String, Option<String>>,
    base: Option<BTreeMap<String, String>>,
    last_background: Option<i32>,
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
            last_background: None,
        }
    }

    pub fn from_map(map: BTreeMap<String, String>) -> Self {
        Self {
            overlay: FxHashMap::default(),
            base: Some(map),
            last_background: None,
        }
    }

    pub fn set_last_background(&mut self, pid: i32) {
        self.last_background = Some(pid);
    }

    pub fn get(&self, key: &str) -> Option<String> {
        if key == "!" {
            return self.last_background.map(|pid| pid.to_string());
        }
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
