use std::collections::HashMap;
use std::sync::Mutex;
use std::time::SystemTime;

use crate::sinks::Value;

/// Last-seen value per topic, shared with the optional dashboard. Kept
/// separate from `Dispatcher` so it has no bearing on dispatch/filter logic
/// or its tests.
#[derive(Default)]
pub struct StatusStore {
    last_seen: Mutex<HashMap<String, (Value, SystemTime)>>,
}

impl StatusStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&self, topic: &str, value: &Value) {
        self.last_seen
            .lock()
            .unwrap()
            .insert(topic.to_string(), (value.clone(), SystemTime::now()));
    }

    pub fn snapshot(&self) -> Vec<(String, Value, SystemTime)> {
        self.last_seen
            .lock()
            .unwrap()
            .iter()
            .map(|(topic, (value, seen))| (topic.clone(), value.clone(), *seen))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_snapshots_last_value() {
        let store = StatusStore::new();
        store.record("plant/a", &Value::Int(1));
        store.record("plant/a", &Value::Int(2));
        store.record("plant/b", &Value::Bool(true));

        let snapshot = store.snapshot();
        assert_eq!(snapshot.len(), 2);
        let a = snapshot.iter().find(|(t, ..)| t == "plant/a").unwrap();
        assert_eq!(a.1, Value::Int(2));
    }
}
