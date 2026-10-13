//! Emulation of the layout of Boost.Unordered's open-addressing ("FOA") containers as used by
//! `boost::concurrent_flat_map` / `boost::concurrent_flat_set` (Boost 1.86, 64-bit), so that
//! iteration (`visit_all`) order is reproduced exactly.
//!
//! Layout: an array of power-of-two many groups of 15 slots; the last slot of the last group is a
//! sentinel. A key's hash is the user hash post-mixed with `mulx` (integral hashes are not
//! avalanching); its home group is `hash >> size_index`; groups are probed quadratically and the
//! element goes into the lowest free slot of the first group with room, setting the group's
//! overflow bit `hash % 8` for every full group passed. The table grows (rehashing the elements in
//! iteration order) when an insertion would exceed the maximum load. Erasure frees the slot and
//! lowers the maximum load if the slot's group has the overflow bit of the erased element set
//! (anti-drift). Only the insertion path of the concurrent containers is emulated (the new element
//! is inserted after a growth rehash, unlike the non-concurrent containers).

const N: usize = 15;
const MLF: f32 = 0.875;
const AVAILABLE: u8 = 0;
const SENTINEL: u8 = 1;

/// `boost::unordered::detail::mulx` for 64-bit `size_t`.
#[inline]
pub fn mulx(x: u64) -> u64 {
    let r = (x as u128) * (0x9E3779B97F4A7C15u64 as u128);
    (r as u64) ^ ((r >> 64) as u64)
}

#[inline]
fn reduced_hash(hash: u64) -> u8 {
    let b = hash as u8;
    match b {
        0 => 8,
        1 => 9,
        x => x,
    }
}

/// `pow2_size_policy::size_index(n)`.
#[inline]
fn size_index(n: usize) -> u32 {
    if n <= 2 {
        63
    } else {
        64 - (64 - ((n - 1) as u64).leading_zeros())
    }
}

/// `size_index_for<group15, pow2_size_policy>(n)`.
#[inline]
fn size_index_for(n: usize) -> u32 {
    size_index(n / N + 1)
}

#[derive(Clone, Debug)]
struct Group {
    meta: [u8; N],
    overflow: u8,
}

impl Group {
    fn new() -> Self {
        Group { meta: [AVAILABLE; N], overflow: 0 }
    }
    #[inline]
    fn first_available(&self) -> Option<usize> {
        self.meta.iter().position(|&m| m == AVAILABLE)
    }
    #[inline]
    fn is_not_overflowed(&self, hash: u64) -> bool {
        self.overflow & (1u8 << (hash % 8)) == 0
    }
    #[inline]
    fn mark_overflow(&mut self, hash: u64) {
        self.overflow |= 1u8 << (hash % 8);
    }
}

#[derive(Clone, Debug)]
pub struct FoaMap<K: PartialEq, V> {
    groups: Vec<Group>,
    /// `groups.len() * N` slots; `None` while capacity is 0 (dummy groups).
    slots: Vec<Option<(K, V)>>,
    size_index: u32,
    size: usize,
    ml: usize,
    hash: fn(&K) -> u64,
}

impl<K: PartialEq, V> FoaMap<K, V> {
    /// An empty container (capacity 0, like a default-constructed one).
    pub fn new(hash: fn(&K) -> u64) -> Self {
        FoaMap { groups: Vec::new(), slots: Vec::new(), size_index: size_index_for(0), size: 0, ml: 0, hash }
    }

    pub fn len(&self) -> usize {
        self.size
    }

    pub fn is_empty(&self) -> bool {
        self.size == 0
    }

    pub fn capacity(&self) -> usize {
        if self.groups.is_empty() {
            0
        } else {
            self.groups.len() * N - 1
        }
    }

    #[inline]
    fn hash_for(&self, k: &K) -> u64 {
        mulx((self.hash)(k))
    }

    #[inline]
    fn position_for(&self, hash: u64) -> usize {
        (hash >> self.size_index) as usize
    }

    fn initial_max_load(capacity: usize) -> usize {
        let small_capacity = 2 * N - 1;
        if capacity <= small_capacity {
            capacity
        } else {
            (MLF * capacity as f32) as usize
        }
    }

    /// Slot index of `k`, if present.
    fn find_slot(&self, k: &K, hash: u64) -> Option<usize> {
        if self.groups.is_empty() {
            return None;
        }
        let mask = self.groups.len() - 1;
        let rh = reduced_hash(hash);
        let mut pos = self.position_for(hash);
        let mut step = 0usize;
        loop {
            let g = &self.groups[pos];
            for n in 0..N {
                if g.meta[n] == rh {
                    if let Some((key, _)) = &self.slots[pos * N + n] {
                        if key == k {
                            return Some(pos * N + n);
                        }
                    }
                }
            }
            if g.is_not_overflowed(hash) {
                return None;
            }
            step += 1;
            pos = (pos + step) & mask;
            if step > mask {
                return None;
            }
        }
    }

    /// `nosize_unchecked_emplace_at` into the given arrays.
    fn place(groups: &mut [Group], slots: &mut [Option<(K, V)>], pos0: usize, hash: u64, kv: (K, V)) -> usize {
        let mask = groups.len() - 1;
        let mut pos = pos0;
        let mut step = 0usize;
        loop {
            if let Some(n) = groups[pos].first_available() {
                groups[pos].meta[n] = reduced_hash(hash);
                slots[pos * N + n] = Some(kv);
                return pos * N + n;
            }
            groups[pos].mark_overflow(hash);
            step += 1;
            pos = (pos + step) & mask;
        }
    }

    /// `unchecked_rehash_for_growth`.
    fn rehash_for_growth(&mut self) {
        let n = ((self.size + self.size / 61 + 1) as f32 / MLF).ceil() as usize;
        let new_index = size_index_for(n);
        let groups_size = 1usize << (64 - new_index);
        let mut groups: Vec<Group> = (0..groups_size).map(|_| Group::new()).collect();
        groups[groups_size - 1].meta[N - 1] = SENTINEL;
        let mut slots: Vec<Option<(K, V)>> = (0..groups_size * N).map(|_| None).collect();
        let old_slots = std::mem::take(&mut self.slots);
        for kv in old_slots.into_iter().flatten() {
            let hash = self.hash_for(&kv.0);
            let pos0 = (hash >> new_index) as usize;
            Self::place(&mut groups, &mut slots, pos0, hash, kv);
        }
        self.groups = groups;
        self.slots = slots;
        self.size_index = new_index;
        self.ml = Self::initial_max_load(self.capacity());
    }

    /// The concurrent containers' insertion: if `k` exists, `visit` is called on its value and
    /// false is returned; otherwise `(k, v)` is inserted (growing first if needed).
    pub fn emplace_or_visit<F: FnOnce(&mut V)>(&mut self, k: K, v: V, visit: F) -> bool {
        let hash = self.hash_for(&k);
        if let Some(slot) = self.find_slot(&k, hash) {
            visit(&mut self.slots[slot].as_mut().unwrap().1);
            return false;
        }
        if self.size + 1 > self.ml {
            // rehash_if_full (size == ml here)
            self.rehash_for_growth();
        }
        let pos0 = self.position_for(hash);
        Self::place(&mut self.groups, &mut self.slots, pos0, hash, (k, v));
        self.size += 1;
        true
    }

    /// `insert` / `emplace` (no visitation).
    pub fn insert(&mut self, k: K, v: V) -> bool {
        self.emplace_or_visit(k, v, |_| {})
    }

    /// `insert_or_assign`.
    pub fn insert_or_assign(&mut self, k: K, v: V) -> bool
    where
        V: Clone,
    {
        let v2 = v.clone();
        self.emplace_or_visit(k, v, move |old| *old = v2)
    }

    pub fn get(&self, k: &K) -> Option<&V> {
        let hash = self.hash_for(k);
        self.find_slot(k, hash).map(|s| &self.slots[s].as_ref().unwrap().1)
    }

    pub fn get_mut(&mut self, k: &K) -> Option<&mut V> {
        let hash = self.hash_for(k);
        self.find_slot(k, hash).map(move |s| &mut self.slots[s].as_mut().unwrap().1)
    }

    pub fn contains(&self, k: &K) -> bool {
        let hash = self.hash_for(k);
        self.find_slot(k, hash).is_some()
    }

    fn erase_slot(&mut self, slot: usize) {
        let pos = slot / N;
        let n = slot % N;
        let g = &mut self.groups[pos];
        let maybe_caused_overflow = !g.is_not_overflowed(g.meta[n] as u64);
        self.ml -= maybe_caused_overflow as usize;
        g.meta[n] = AVAILABLE;
        self.slots[slot] = None;
        self.size -= 1;
    }

    /// `erase(k)`.
    pub fn erase(&mut self, k: &K) -> usize {
        let hash = self.hash_for(k);
        match self.find_slot(k, hash) {
            Some(s) => {
                self.erase_slot(s);
                1
            }
            None => 0,
        }
    }

    /// `erase_if(f)` (over all elements, in iteration order).
    pub fn erase_if<F: FnMut(&K, &V) -> bool>(&mut self, mut f: F) -> usize {
        let order: Vec<usize> = self.slot_order();
        let mut res = 0;
        for s in order {
            let erase = {
                let (k, v) = self.slots[s].as_ref().unwrap();
                f(k, v)
            };
            if erase {
                self.erase_slot(s);
                res += 1;
            }
        }
        res
    }

    fn slot_order(&self) -> Vec<usize> {
        // Slots are group-major, so slot index order is iteration order (the sentinel slot never
        // holds an element).
        (0..self.slots.len()).filter(|&s| self.slots[s].is_some()).collect()
    }

    /// Elements in `visit_all` order.
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.slots.iter().filter_map(|s| s.as_ref().map(|(k, v)| (k, v)))
    }

    /// Elements in `visit_all` order, mutable values.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&K, &mut V)> {
        self.slots.iter_mut().filter_map(|s| s.as_mut().map(|(k, v)| (&*k, v)))
    }
}

/// `boost::concurrent_flat_set` layout emulation.
#[derive(Clone, Debug)]
pub struct FoaSet<K: PartialEq> {
    map: FoaMap<K, ()>,
}

impl<K: PartialEq> FoaSet<K> {
    pub fn new(hash: fn(&K) -> u64) -> Self {
        FoaSet { map: FoaMap::new(hash) }
    }
    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
    /// `insert` / `emplace`: true if inserted.
    pub fn insert(&mut self, k: K) -> bool {
        self.map.insert(k, ())
    }
    pub fn contains(&self, k: &K) -> bool {
        self.map.contains(k)
    }
    pub fn erase(&mut self, k: &K) -> usize {
        self.map.erase(k)
    }
    pub fn erase_if<F: FnMut(&K) -> bool>(&mut self, mut f: F) -> usize {
        self.map.erase_if(|k, _| f(k))
    }
    pub fn iter(&self) -> impl Iterator<Item = &K> {
        self.map.iter().map(|(k, _)| k)
    }
}
