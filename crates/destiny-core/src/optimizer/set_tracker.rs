//! DIM process-worker/set-tracker.ts:1-190 (`HeapSetTracker`, `encodeStatMix`).
//! Ghostlink deviations from DIM's ordering, so the retained sets never depend on
//! the order sets are visited (item order, slicing, thread count):
//! * `could_insert` admits a total that ties the worst retained set (DIM
//!   set-tracker.ts:46-48 rejects it and keeps the first found);
//! * ties on every DIM key are broken on the item ids (`tie_break`, smaller wins),
//!   which makes the order total; DIM leaves them to heap position.
use super::DesiredStatRange;
use std::cmp::Ordering;

/// DIM process.ts:34 `RETURNED_ARMOR_SETS`.
pub const RETURNED_ARMOR_SETS: usize = 200;

/// Item ranks of the five pieces in bucket order, then the chosen exotic tuning mod
/// (0 = none): a deterministic stand-in for DIM's item order.
pub type TieBreak = ([u32; 5], u32);

/// DIM `HeapEntry<T>`; `data` is everything the tracker does not order on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HeapEntry<T> {
    /// Sum of enabled stat values.
    pub enabled_stats_total: i32,
    /// Sum of all stat values, including ignored and maxed stats.
    pub stats_total: i32,
    /// Enabled stats packed 8 bits each in priority order (`encode_stat_mix`).
    pub stat_mix: u64,
    pub power: i32,
    pub tie_break: TieBreak,
    pub data: T,
}

impl<T> HeapEntry<T> {
    /// Best-first order: DIM `isWorse` (set-tracker.ts:21-32) with `tie_break` last.
    fn rank_cmp(&self, other: &Self) -> Ordering {
        self.enabled_stats_total
            .cmp(&other.enabled_stats_total)
            .then(self.stat_mix.cmp(&other.stat_mix))
            .then(self.stats_total.cmp(&other.stats_total))
            .then(self.power.cmp(&other.power))
            .then_with(|| other.tie_break.cmp(&self.tie_break))
    }
    fn is_worse(&self, other: &Self) -> bool {
        self.rank_cmp(other) == Ordering::Less
    }
}

/// DIM `HeapSetTracker`: a min-heap whose root is the worst retained set.
#[derive(Debug)]
pub struct HeapSetTracker<T> {
    heap: Vec<HeapEntry<T>>,
    capacity: usize,
}

impl<T> HeapSetTracker<T> {
    pub fn new(capacity: usize) -> Self {
        Self {
            heap: Vec::new(),
            capacity,
        }
    }

    /// Could a set with this enabled-stat total still be retained? A tie with the
    /// worst retained set still needs the full comparison (see the module docs).
    pub fn could_insert(&self, total: i32) -> bool {
        self.heap.len() < self.capacity || total >= self.heap[0].enabled_stats_total
    }

    /// Returns true only when the set was added without trimming, like DIM.
    pub fn insert(&mut self, entry: HeapEntry<T>) -> bool {
        if self.heap.len() < self.capacity {
            self.heap.push(entry);
            self.bubble_up(self.heap.len() - 1);
            return true;
        }
        if entry.is_worse(&self.heap[0]) {
            return false;
        }
        self.heap[0] = entry;
        self.bubble_down(0);
        false
    }

    pub fn len(&self) -> usize {
        self.heap.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }

    /// DIM `getArmorSets`: the retained sets, best first.
    pub fn into_sorted(mut self) -> Vec<HeapEntry<T>> {
        self.heap.sort_by(|a, b| b.rank_cmp(a));
        self.heap
    }

    fn bubble_up(&mut self, mut index: usize) {
        while index > 0 {
            let parent = (index - 1) / 2;
            if !self.heap[index].is_worse(&self.heap[parent]) {
                break;
            }
            self.heap.swap(index, parent);
            index = parent;
        }
    }

    fn bubble_down(&mut self, mut index: usize) {
        loop {
            let mut smallest = index;
            for child in [2 * index + 1, 2 * index + 2] {
                if child < self.heap.len() && self.heap[child].is_worse(&self.heap[smallest]) {
                    smallest = child;
                }
            }
            if smallest == index {
                break;
            }
            self.heap.swap(index, smallest);
            index = smallest;
        }
    }
}

/// DIM `encodeStatMix` (set-tracker.ts:166-178): non-ignored stats only, 8 bits each,
/// clamped to 255, in priority order.
pub fn encode_stat_mix(stats: &[i32; 6], desired: &[DesiredStatRange]) -> u64 {
    let mut encoded = 0;
    for (value, range) in stats.iter().zip(desired) {
        if range.max_stat > 0 {
            encoded = encoded * 256 + (*value).clamp(0, 255) as u64;
        }
    }
    encoded
}
