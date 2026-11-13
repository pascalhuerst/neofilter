//! Lock-free single-producer / single-consumer ring of stereo `f32` frames.
//!
//! The audio thread is the producer and must never wait, so when the consumer
//! falls behind the **newest** frames are dropped (the part that does not fit)
//! and counted in an overrun counter that the GUI shows. Samples are stored as
//! `AtomicU32` bit patterns, so no `unsafe` is needed; the head/tail indices
//! use release/acquire ordering.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

struct Ring {
    l: Box<[AtomicU32]>,
    r: Box<[AtomicU32]>,
    mask: u64,
    /// Total frames ever written (producer).
    head: AtomicU64,
    /// Total frames ever read (consumer).
    tail: AtomicU64,
    /// Frames dropped because the ring was full.
    overrun: AtomicU64,
}

/// Audio-thread end.
pub struct Producer {
    ring: Arc<Ring>,
    head: u64,
}

/// Analysis-thread end.
pub struct Consumer {
    ring: Arc<Ring>,
    tail: u64,
}

/// Create a ring holding at least `capacity_frames` frames (rounded up to a
/// power of two).
pub fn channel(capacity_frames: usize) -> (Producer, Consumer) {
    let cap = capacity_frames.max(2).next_power_of_two();
    let mk = || -> Box<[AtomicU32]> { (0..cap).map(|_| AtomicU32::new(0)).collect() };
    let ring = Arc::new(Ring {
        l: mk(),
        r: mk(),
        mask: cap as u64 - 1,
        head: AtomicU64::new(0),
        tail: AtomicU64::new(0),
        overrun: AtomicU64::new(0),
    });
    (
        Producer {
            ring: ring.clone(),
            head: 0,
        },
        Consumer { ring, tail: 0 },
    )
}

impl Producer {
    /// Capacity in frames.
    pub fn capacity(&self) -> usize {
        self.ring.mask as usize + 1
    }

    /// Push frames; returns how many were stored. Never blocks, never
    /// allocates. The frames that do not fit are dropped and counted.
    pub fn push(&mut self, l: &[f32], r: &[f32]) -> usize {
        let n = l.len().min(r.len());
        let cap = self.ring.mask + 1;
        let tail = self.ring.tail.load(Ordering::Acquire);
        let free = cap - (self.head - tail);
        let w = (n as u64).min(free) as usize;
        for i in 0..w {
            let idx = ((self.head + i as u64) & self.ring.mask) as usize;
            self.ring.l[idx].store(l[i].to_bits(), Ordering::Relaxed);
            self.ring.r[idx].store(r[i].to_bits(), Ordering::Relaxed);
        }
        self.head += w as u64;
        self.ring.head.store(self.head, Ordering::Release);
        if w < n {
            self.ring
                .overrun
                .fetch_add((n - w) as u64, Ordering::Relaxed);
        }
        w
    }
}

impl Consumer {
    /// Frames currently available.
    pub fn available(&self) -> usize {
        (self.ring.head.load(Ordering::Acquire) - self.tail) as usize
    }

    /// Pop up to `l.len().min(r.len())` frames; returns the count.
    pub fn pop(&mut self, l: &mut [f32], r: &mut [f32]) -> usize {
        let n = self.available().min(l.len()).min(r.len());
        for i in 0..n {
            let idx = ((self.tail + i as u64) & self.ring.mask) as usize;
            l[i] = f32::from_bits(self.ring.l[idx].load(Ordering::Relaxed));
            r[i] = f32::from_bits(self.ring.r[idx].load(Ordering::Relaxed));
        }
        self.tail += n as u64;
        self.ring.tail.store(self.tail, Ordering::Release);
        n
    }

    /// Total frames dropped by the producer so far.
    pub fn overruns(&self) -> u64 {
        self.ring.overrun.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fifo_order_and_wraparound() {
        let (mut p, mut c) = channel(8);
        let mut next = 0.0f32;
        let mut expect = 0.0f32;
        for _ in 0..50 {
            let l: Vec<f32> = (0..5).map(|i| next + i as f32).collect();
            let r: Vec<f32> = l.iter().map(|v| -v).collect();
            assert_eq!(p.push(&l, &r), 5);
            next += 5.0;
            let (mut a, mut b) = ([0.0; 5], [0.0; 5]);
            assert_eq!(c.pop(&mut a, &mut b), 5);
            for i in 0..5 {
                assert_eq!(a[i], expect);
                assert_eq!(b[i], -expect);
                expect += 1.0;
            }
        }
        assert_eq!(c.overruns(), 0);
    }

    #[test]
    fn overrun_drops_newest_and_counts() {
        let (mut p, mut c) = channel(8);
        let x: Vec<f32> = (0..20).map(|i| i as f32).collect();
        assert_eq!(p.push(&x, &x), 8);
        assert_eq!(c.overruns(), 12);
        let (mut a, mut b) = ([0.0; 32], [0.0; 32]);
        assert_eq!(c.pop(&mut a, &mut b), 8);
        assert_eq!(&a[..8], &x[..8]);
        // space again
        assert_eq!(p.push(&x[..4], &x[..4]), 4);
    }
}
