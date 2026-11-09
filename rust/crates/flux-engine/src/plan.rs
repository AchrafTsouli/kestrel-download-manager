//! The segment table. Pure: no I/O, so it is tested directly (including a random simulation).
//!
//! A segment covers `start..=end` of the file; `done` bytes from `start` are on disk. A free connection
//! takes an idle unfinished segment, or else splits the largest remaining active one in half.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Segment {
    pub start: u64,
    /// Inclusive end. `None` when the size is unknown (one connection, read to the end).
    pub end: Option<u64>,
    /// Bytes written from `start` and confirmed on disk (resume point).
    pub done: u64,
    #[serde(skip)]
    pub active: bool,
}

impl Segment {
    pub fn len(&self) -> Option<u64> {
        self.end.map(|end| end - self.start + 1)
    }
    pub fn remaining(&self) -> Option<u64> {
        self.len().map(|len| len.saturating_sub(self.done))
    }
    pub fn finished(&self) -> bool {
        self.remaining() == Some(0)
    }
    /// The next byte to request.
    pub fn position(&self) -> u64 {
        self.start + self.done
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub segments: Vec<Segment>,
    /// How many times a segment was split while downloading.
    pub splits: u32,
}

/// What a free connection should do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Claim {
    pub index: usize,
    pub split: bool,
}

impl Plan {
    /// `connections` equal parts of a file of `total` bytes (one part when ranges are not allowed or the size is unknown).
    pub fn new(total: Option<u64>, ranged: bool, connections: usize, min_part: u64) -> Plan {
        let Some(total) = total.filter(|_| ranged) else {
            return Plan { segments: vec![Segment { start: 0, end: total.map(|t| t.saturating_sub(1)).filter(|_| total.is_some_and(|t| t > 0)), done: 0, active: false }], splits: 0 };
        };
        if total == 0 {
            return Plan { segments: vec![Segment { start: 0, end: None, done: 0, active: false }], splits: 0 };
        }
        let count = (connections.max(1) as u64).min((total / min_part.max(1)).max(1));
        let chunk = total / count;
        let segments = (0..count)
            .map(|i| Segment { start: i * chunk, end: Some(if i == count - 1 { total - 1 } else { (i + 1) * chunk - 1 }), done: 0, active: false })
            .collect();
        Plan { segments, splits: 0 }
    }

    pub fn written(&self) -> u64 {
        self.segments.iter().map(|s| s.done).sum()
    }
    pub fn complete(&self, total: Option<u64>) -> bool {
        match total {
            Some(total) => {
                let mut ordered: Vec<&Segment> = self.segments.iter().collect();
                ordered.sort_by_key(|s| s.start);
                let mut next = 0;
                for segment in ordered {
                    if segment.start != next || !segment.finished() {
                        return false;
                    }
                    next = segment.end.unwrap() + 1;
                }
                next == total
            }
            None => false,
        }
    }

    /// The next job for a free connection, marking it active. `None` when nothing is left to give.
    pub fn claim(&mut self, min_split: u64, max_segments: usize) -> Option<Claim> {
        if let Some(index) = self.segments.iter().position(|s| !s.active && !s.finished()) {
            self.segments[index].active = true;
            return Some(Claim { index, split: false });
        }
        if self.segments.len() >= max_segments {
            return None;
        }
        let (index, remaining) = self
            .segments
            .iter()
            .enumerate()
            .filter(|(_, s)| s.active)
            .filter_map(|(i, s)| s.remaining().map(|r| (i, r)))
            .max_by_key(|(_, r)| *r)?;
        if remaining < (min_split * 2).max(1 << 20) {
            return None;
        }
        let old = &mut self.segments[index];
        let middle = old.position() + remaining / 2;
        let tail = Segment { start: middle, end: old.end, done: 0, active: true };
        old.end = Some(middle - 1);
        self.segments.push(tail);
        self.splits += 1;
        Some(Claim { index: self.segments.len() - 1, split: true })
    }

    pub fn release(&mut self, index: usize) {
        if let Some(segment) = self.segments.get_mut(index) {
            segment.active = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_parts_cover_the_file() {
        let plan = Plan::new(Some(10_000_001), true, 4, 1);
        assert_eq!(plan.segments.len(), 4);
        assert_eq!(plan.segments[3].end, Some(10_000_000));
        assert_eq!(Plan::new(Some(5000), false, 8, 1).segments.len(), 1);
        assert_eq!(Plan::new(Some(3 << 20), true, 16, 1 << 20).segments.len(), 3, "small files get fewer parts");
        assert_eq!(Plan::new(None, true, 8, 1).segments[0].end, None);
    }

    #[test]
    fn free_connections_split_the_largest_remaining_part() {
        let mut plan = Plan::new(Some(100 << 20), true, 2, 1);
        assert_eq!(plan.claim(1, 64), Some(Claim { index: 0, split: false }));
        assert_eq!(plan.claim(1, 64), Some(Claim { index: 1, split: false }));
        plan.segments[0].done = 40 << 20;
        let claim = plan.claim(1, 64).unwrap();
        assert!(claim.split);
        assert_eq!(plan.segments[2].start, plan.segments[1].start + (25 << 20));
        assert_eq!(plan.segments[1].end, Some(plan.segments[2].start - 1));
        assert!(plan.claim(1, 3).is_none(), "segment cap respected");
    }

    /// Random simulation: workers take turns writing random amounts; the file must end fully covered
    /// with no byte written twice, whatever the order.
    #[test]
    fn random_simulation_always_covers_every_byte_once() {
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut next = move || { seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17; seed };
        for _ in 0..200 {
            let total = 1 + next() % (64 << 20);
            let mut plan = Plan::new(Some(total), true, 1 + (next() % 8) as usize, 1);
            let mut covered = vec![0u8; 0];
            let mut written: Vec<(u64, u64)> = Vec::new();
            let mut workers: Vec<usize> = Vec::new();
            while let Some(c) = plan.claim(256 << 10, 64) { workers.push(c.index); }
            let mut rounds = 0;
            while !workers.is_empty() && rounds < 100_000 {
                rounds += 1;
                let w = (next() % workers.len() as u64) as usize;
                let index = workers[w];
                let seg = &mut plan.segments[index];
                let remaining = seg.remaining().unwrap();
                let step = (1 + next() % (4 << 20)).min(remaining);
                written.push((seg.position(), step));
                seg.done += step;
                if seg.finished() {
                    plan.release(index);
                    workers.swap_remove(w);
                    if let Some(c) = plan.claim(256 << 10, 64) { workers.push(c.index); }
                }
            }
            assert!(plan.complete(Some(total)), "not complete for total {total}");
            written.sort();
            let mut end = 0;
            for (start, len) in written { assert_eq!(start, end, "gap or overlap"); end = start + len; }
            assert_eq!(end, total);
            let _ = &mut covered;
        }
    }
}
