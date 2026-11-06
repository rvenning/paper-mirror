//! A byte-budgeted LRU cache for rendered page images and thumbnails.
//!
//! Entries are evicted least-recently-used first once the total cost exceeds the
//! budget. The cache holds at most a few hundred entries, so a `Vec` scan beats a
//! linked hash map and keeps the code dependency-free.

pub struct ByteLru<K, V> {
    budget: usize,
    used: usize,
    tick: u64,
    entries: Vec<Entry<K, V>>,
}

struct Entry<K, V> {
    key: K,
    value: V,
    cost: usize,
    used_at: u64,
}

impl<K: PartialEq + Clone, V> ByteLru<K, V> {
    pub fn new(budget: usize) -> Self {
        Self { budget, used: 0, tick: 0, entries: Vec::new() }
    }

    pub fn budget(&self) -> usize {
        self.budget
    }

    pub fn used(&self) -> usize {
        self.used
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&mut self, key: &K) -> Option<&V> {
        self.tick += 1;
        let tick = self.tick;
        self.entries.iter_mut().find(|e| e.key == *key).map(|e| {
            e.used_at = tick;
            &e.value
        })
    }

    /// Looks up without touching recency.
    pub fn peek(&self, key: &K) -> Option<&V> {
        self.entries.iter().find(|e| e.key == *key).map(|e| &e.value)
    }

    pub fn contains(&self, key: &K) -> bool {
        self.entries.iter().any(|e| e.key == *key)
    }

    /// Inserts or replaces, then evicts until within budget. An entry larger than the
    /// whole budget is still kept (alone) so the visible page can always be shown.
    pub fn insert(&mut self, key: K, value: V, cost: usize) {
        self.remove(&key);
        self.tick += 1;
        self.used += cost;
        self.entries.push(Entry { key: key.clone(), value, cost, used_at: self.tick });
        while self.used > self.budget && self.entries.len() > 1 {
            let (idx, _) = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e.key != key)
                .min_by_key(|(_, e)| e.used_at)
                .expect("more than one entry");
            let e = self.entries.swap_remove(idx);
            self.used -= e.cost;
        }
    }

    pub fn remove(&mut self, key: &K) -> Option<V> {
        let idx = self.entries.iter().position(|e| e.key == *key)?;
        let e = self.entries.swap_remove(idx);
        self.used -= e.cost;
        Some(e.value)
    }

    pub fn retain(&mut self, mut keep: impl FnMut(&K) -> bool) {
        let mut used = 0;
        self.entries.retain(|e| {
            let k = keep(&e.key);
            if k {
                used += e.cost;
            }
            k
        });
        self.used = used;
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.used = 0;
    }

    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.entries.iter().map(|e| &e.key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_least_recently_used_within_budget() {
        let mut c = ByteLru::new(100);
        c.insert(1, "a", 40);
        c.insert(2, "b", 40);
        assert!(c.get(&1).is_some()); // 1 is now more recent than 2
        c.insert(3, "c", 40);
        assert!(c.contains(&1) && c.contains(&3) && !c.contains(&2));
        assert_eq!(c.used(), 80);
    }

    #[test]
    fn oversized_entry_is_kept_alone() {
        let mut c = ByteLru::new(10);
        c.insert(1, (), 5);
        c.insert(2, (), 50);
        assert_eq!(c.len(), 1);
        assert!(c.contains(&2));
        assert_eq!(c.used(), 50);
    }

    #[test]
    fn replace_updates_cost_and_retain_recounts() {
        let mut c = ByteLru::new(100);
        c.insert(1, (), 30);
        c.insert(1, (), 10);
        assert_eq!(c.used(), 10);
        c.insert(2, (), 20);
        c.retain(|k| *k == 2);
        assert_eq!(c.used(), 20);
        assert_eq!(c.remove(&2), Some(()));
        assert_eq!(c.used(), 0);
        assert!(c.is_empty());
    }
}
