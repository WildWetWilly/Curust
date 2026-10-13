//! Emulation of libstdc++ (GCC 13) `std::unordered_set` iteration order: a singly linked node list
//! with buckets pointing at the node *before* the bucket's first node, prime-number bucket counts
//! (`_Prime_rehash_policy`, max load factor 1.0) and insertion at the beginning of a bucket.
//!
//! Only the operations needed by the engine are provided: insert (`emplace`), erase by key,
//! `merge`, lookup and iteration in container order.

/// `__prime_list` from libstdc++ (64 bit), starting at `__prime_list + 6`.
static PRIMES: &[u64] = &[17,19,23,29,31,37,41,43,47,53,59,61,67,71,73,79,83,89,97,103,109,113,127,137,139,149,157,167,179,193,199,211,227,241,257,277,293,313,337,359,383,409,439,467,503,541,577,619,661,709,761,823,887,953,1031,1109,1193,1289,1381,1493,1613,1741,1879,2029,2179,2357,2549,2753,2971,3209,3469,3739,4027,4349,4703,5087,5503,5953,6427,6949,7517,8123,8783,9497,10273,11113,12011,12983,14033,15173,16411,17749,19183,20753,22447,24281,26267,28411,30727,33223,35933,38873,42043,45481,49201,53201,57557,62233,67307,72817,78779,85229,92203,99733,107897,116731,126271,136607,147793,159871,172933,187091,202409,218971,236897,256279,277261,299951,324503,351061,379787,410857,444487,480881,520241,562841,608903,658753,712697,771049,834181,902483,976369,1056323,1142821,1236397,1337629,1447153,1565659,1693859,1832561,1982627,2144977,2320627,2510653,2716249,2938679,3179303,3439651,3721303,4026031,4355707,4712381,5098259,5515729,5967347,6456007,6984629,7556579,8175383,8844859,9569143,10352717,11200489,12117689,13109983,14183539,15345007,16601593,17961079,19431899,21023161,22744717,24607243,26622317,28802401,31160981,33712729,36473443,39460231,42691603,46187573,49969847,54061849,58488943,63278561,68460391,74066549,80131819,86693767,93793069,101473717,109783337,118773397,128499677,139022417,150406843,162723577,176048909,190465427,206062531,222936881,241193053,260944219,282312799,305431229,330442829,357502601,386778277,418451333,452718089,489790921,529899637,573292817,620239453,671030513,725980837,785430967,849749479,919334987,994618837,1076067617,1164186217,1259520799,1362662261,1474249943,1594975441,1725587117,1866894511,2019773507,2185171673,2364114217,2557710269,2767159799,2993761039,3238918481,3504151727,3791104843,4101556399,4294967291,6442450933,8589934583,12884901857,17179869143,25769803693,34359738337,51539607367,68719476731,103079215087,137438953447,206158430123,274877906899,412316860387,549755813881,824633720731,1099511627689,1649267441579,2199023255531,3298534883309,4398046511093,6597069766607,8796093022151,13194139533241,17592186044399,26388279066581,35184372088777,52776558133177,70368744177643,105553116266399,140737488355213,211106232532861,281474976710597,562949953421231,1125899906842597,2251799813685119,4503599627370449,9007199254740881,18014398509481951,36028797018963913,72057594037927931,144115188075855859,288230376151711717,576460752303423433,1152921504606846883,2305843009213693951,4611686018427387847,9223372036854775783,18446744073709551557];
/// `__fast_bkt`
const FAST_BKT: [usize; 14] = [2, 2, 2, 3, 5, 5, 7, 7, 11, 11, 11, 11, 13, 13];

const NONE: usize = usize::MAX;
const BEFORE_BEGIN: usize = usize::MAX - 1;

#[derive(Clone, Debug)]
struct RehashPolicy {
    max_load_factor: f32,
    next_resize: usize,
}

impl RehashPolicy {
    fn next_bkt(&mut self, n: usize) -> usize {
        if n < FAST_BKT.len() {
            if n == 0 {
                return 1;
            }
            self.next_resize = (FAST_BKT[n] as f64 * self.max_load_factor as f64).floor() as usize;
            return FAST_BKT[n];
        }
        let last = PRIMES.len() - 1;
        let idx = PRIMES[..last].partition_point(|&p| (p as usize) < n);
        if idx == last {
            self.next_resize = usize::MAX;
        } else {
            self.next_resize = (PRIMES[idx] as f64 * self.max_load_factor as f64).floor() as usize;
        }
        PRIMES[idx] as usize
    }

    fn need_rehash(&mut self, n_bkt: usize, n_elt: usize, n_ins: usize) -> Option<usize> {
        if n_elt + n_ins > self.next_resize {
            let min_bkts = (n_elt + n_ins).max(if self.next_resize != 0 { 0 } else { 11 }) as f64 / self.max_load_factor as f64;
            if min_bkts >= n_bkt as f64 {
                return Some(self.next_bkt((min_bkts.floor() as usize + 1).max(n_bkt * 2)));
            }
            self.next_resize = (n_bkt as f64 * self.max_load_factor as f64).floor() as usize;
        }
        None
    }
}

#[derive(Clone, Debug)]
struct Node<K> {
    key: K,
    next: usize,
}

/// libstdc++'s `std::unordered_set<K>` with a given `std::hash` function.
#[derive(Clone, Debug)]
pub struct GnuSet<K: Clone + PartialEq> {
    nodes: Vec<Node<K>>,
    free: Vec<usize>,
    /// Each bucket holds the node before its first node (`BEFORE_BEGIN` for the list head), or `NONE`.
    buckets: Vec<usize>,
    before_begin_next: usize,
    len: usize,
    policy: RehashPolicy,
    hash: fn(&K) -> u64,
}

impl<K: Clone + PartialEq> GnuSet<K> {
    pub fn new(hash: fn(&K) -> u64) -> Self {
        GnuSet {
            nodes: Vec::new(),
            free: Vec::new(),
            buckets: vec![NONE],
            before_begin_next: NONE,
            len: 0,
            policy: RehashPolicy { max_load_factor: 1.0, next_resize: 0 },
            hash,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn bucket_count(&self) -> usize {
        self.buckets.len()
    }

    #[inline]
    fn next_of(&self, prev: usize) -> usize {
        if prev == BEFORE_BEGIN {
            self.before_begin_next
        } else {
            self.nodes[prev].next
        }
    }

    #[inline]
    fn set_next(&mut self, prev: usize, next: usize) {
        if prev == BEFORE_BEGIN {
            self.before_begin_next = next;
        } else {
            self.nodes[prev].next = next;
        }
    }

    #[inline]
    fn bucket_index_key(&self, k: &K) -> usize {
        ((self.hash)(k) % self.buckets.len() as u64) as usize
    }

    #[inline]
    fn bucket_index(&self, node: usize) -> usize {
        self.bucket_index_key(&self.nodes[node].key)
    }

    /// `_M_find_before_node`
    fn find_before_node(&self, bkt: usize, k: &K) -> Option<usize> {
        let mut prev = self.buckets[bkt];
        if prev == NONE {
            return None;
        }
        let mut p = self.next_of(prev);
        loop {
            if self.nodes[p].key == *k {
                return Some(prev);
            }
            let next = self.nodes[p].next;
            if next == NONE || self.bucket_index(next) != bkt {
                break;
            }
            prev = p;
            p = next;
        }
        None
    }

    pub fn contains(&self, k: &K) -> bool {
        if self.len == 0 {
            return false;
        }
        self.find_before_node(self.bucket_index_key(k), k).is_some()
    }

    fn rehash(&mut self, n: usize) {
        let mut new_buckets = vec![NONE; n];
        let mut p = self.before_begin_next;
        self.before_begin_next = NONE;
        let mut bbegin_bkt = 0usize;
        while p != NONE {
            let next = self.nodes[p].next;
            let bkt = ((self.hash)(&self.nodes[p].key) % n as u64) as usize;
            if new_buckets[bkt] == NONE {
                self.nodes[p].next = self.before_begin_next;
                self.before_begin_next = p;
                new_buckets[bkt] = BEFORE_BEGIN;
                if self.nodes[p].next != NONE {
                    new_buckets[bbegin_bkt] = p;
                }
                bbegin_bkt = bkt;
            } else {
                let prev = new_buckets[bkt];
                let after = if prev == BEFORE_BEGIN { self.before_begin_next } else { self.nodes[prev].next };
                self.nodes[p].next = after;
                if prev == BEFORE_BEGIN {
                    self.before_begin_next = p;
                } else {
                    self.nodes[prev].next = p;
                }
            }
            p = next;
        }
        self.buckets = new_buckets;
    }

    /// `_M_insert_bucket_begin`
    fn insert_bucket_begin(&mut self, bkt: usize, node: usize) {
        let prev = self.buckets[bkt];
        if prev != NONE {
            let after = self.next_of(prev);
            self.nodes[node].next = after;
            self.set_next(prev, node);
        } else {
            self.nodes[node].next = self.before_begin_next;
            self.before_begin_next = node;
            let next = self.nodes[node].next;
            if next != NONE {
                let nb = self.bucket_index(next);
                self.buckets[nb] = node;
            }
            self.buckets[bkt] = BEFORE_BEGIN;
        }
    }

    /// `_M_insert_unique_node`
    fn insert_unique_node(&mut self, mut bkt: usize, node: usize, n_elt: usize) {
        if let Some(n) = self.policy.need_rehash(self.buckets.len(), self.len, n_elt) {
            self.rehash(n);
            bkt = self.bucket_index(node);
        }
        self.insert_bucket_begin(bkt, node);
        self.len += 1;
    }

    fn alloc_node(&mut self, key: K) -> usize {
        if let Some(i) = self.free.pop() {
            self.nodes[i] = Node { key, next: NONE };
            i
        } else {
            self.nodes.push(Node { key, next: NONE });
            self.nodes.len() - 1
        }
    }

    /// `emplace` / `insert`: returns whether the key was inserted.
    pub fn insert(&mut self, key: K) -> bool {
        let bkt = self.bucket_index_key(&key);
        if self.len > 0 && self.find_before_node(bkt, &key).is_some() {
            return false;
        }
        let node = self.alloc_node(key);
        self.insert_unique_node(bkt, node, 1);
        true
    }

    /// Unlink the node after `prev_n` in bucket `bkt` (`_M_erase` / `_M_extract_node`).
    fn unlink(&mut self, bkt: usize, prev_n: usize) -> usize {
        let n = self.next_of(prev_n);
        let n_next = self.nodes[n].next;
        if prev_n == self.buckets[bkt] {
            // _M_remove_bucket_begin
            let next_bkt = if n_next != NONE { self.bucket_index(n_next) } else { 0 };
            if n_next == NONE || next_bkt != bkt {
                if n_next != NONE {
                    self.buckets[next_bkt] = self.buckets[bkt];
                }
                if self.buckets[bkt] == BEFORE_BEGIN {
                    self.before_begin_next = n_next;
                }
                self.buckets[bkt] = NONE;
            }
        } else if n_next != NONE {
            let next_bkt = self.bucket_index(n_next);
            if next_bkt != bkt {
                self.buckets[next_bkt] = prev_n;
            }
        }
        self.set_next(prev_n, n_next);
        self.nodes[n].next = NONE;
        self.len -= 1;
        n
    }

    /// `erase(key)`: returns the number of erased elements.
    pub fn remove(&mut self, k: &K) -> usize {
        if self.len == 0 {
            return 0;
        }
        let bkt = self.bucket_index_key(k);
        let Some(prev_n) = self.find_before_node(bkt, k) else {
            return 0;
        };
        let n = self.unlink(bkt, prev_n);
        self.free.push(n);
        1
    }

    /// The previous node of `node` (`_M_get_previous_node`).
    fn previous_node(&self, bkt: usize, node: usize) -> usize {
        let mut prev = self.buckets[bkt];
        while self.next_of(prev) != node {
            prev = self.next_of(prev);
        }
        prev
    }

    /// `merge(src)`: move the elements of `src` that are not in `self` into `self`.
    pub fn merge(&mut self, src: &mut GnuSet<K>) {
        let mut n_elt = src.len;
        let mut i = src.before_begin_next;
        while i != NONE {
            let pos = i;
            i = src.nodes[i].next;
            let key = src.nodes[pos].key.clone();
            let bkt = self.bucket_index_key(&key);
            if self.len == 0 || self.find_before_node(bkt, &key).is_none() {
                // src.extract(pos)
                let src_bkt = src.bucket_index(pos);
                let prev = src.previous_node(src_bkt, pos);
                let n = src.unlink(src_bkt, prev);
                src.free.push(n);
                let node = self.alloc_node(key);
                self.insert_unique_node(bkt, node, n_elt);
                n_elt = 1;
            } else if n_elt != 1 {
                n_elt -= 1;
            }
        }
    }

    /// Iterate in container order.
    pub fn iter(&self) -> GnuIter<'_, K> {
        GnuIter { set: self, cur: self.before_begin_next }
    }

    pub fn to_vec(&self) -> Vec<K> {
        self.iter().cloned().collect()
    }
}

pub struct GnuIter<'a, K: Clone + PartialEq> {
    set: &'a GnuSet<K>,
    cur: usize,
}

impl<'a, K: Clone + PartialEq> Iterator for GnuIter<'a, K> {
    type Item = &'a K;
    fn next(&mut self) -> Option<&'a K> {
        if self.cur == NONE {
            return None;
        }
        let n = &self.set.nodes[self.cur];
        self.cur = n.next;
        Some(&n.key)
    }
}

/// libstdc++ `std::_Hash_bytes` (64-bit Murmur variant) as used by `std::hash<std::string>`.
pub fn hash_bytes(buf: &[u8], seed: u64) -> u64 {
    const MUL: u64 = (0xc6a4a793u64 << 32) + 0x5bd1e995;
    let shift_mix = |v: u64| v ^ (v >> 47);
    let len = buf.len();
    let len_aligned = len & !7;
    let mut hash = seed ^ (len as u64).wrapping_mul(MUL);
    for chunk in buf[..len_aligned].chunks_exact(8) {
        let data = shift_mix(u64::from_le_bytes(chunk.try_into().unwrap()).wrapping_mul(MUL)).wrapping_mul(MUL);
        hash ^= data;
        hash = hash.wrapping_mul(MUL);
    }
    if len & 7 != 0 {
        let mut data = 0u64;
        for &b in buf[len_aligned..].iter().rev() {
            data = (data << 8) + b as u64;
        }
        hash ^= data;
        hash = hash.wrapping_mul(MUL);
    }
    hash = shift_mix(hash).wrapping_mul(MUL);
    shift_mix(hash)
}

/// libstdc++'s `std::hash<std::string>`.
pub fn string_hash(s: &String) -> u64 {
    hash_bytes(s.as_bytes(), 0xc70f6907)
}

/// FNV-1a over bytes, Microsoft's `std::hash` for integers and strings (`_Fnv1a_append_bytes`).
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 14695981039346656037;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    h
}

/// `std::hash<K>` of the two standard libraries, for the key types the engine hashes.
pub trait StdHash {
    /// libstdc++: the value itself for integers, `_Hash_bytes` for strings.
    fn gnu_hash(&self) -> u64;
    /// Microsoft STL: FNV-1a of the object representation / characters.
    fn msvc_hash(&self) -> u64;
}

macro_rules! std_hash_int {
    ($($t:ty),*) => {$(
        impl StdHash for $t {
            fn gnu_hash(&self) -> u64 {
                // static_cast<size_t>(value): sign extension for signed types
                *self as i128 as u64
            }
            fn msvc_hash(&self) -> u64 {
                fnv1a(&self.to_le_bytes())
            }
        }
    )*};
}
std_hash_int!(i8, u8, i16, u16, i32, u32, i64, u64, usize, isize);

impl StdHash for String {
    fn gnu_hash(&self) -> u64 {
        string_hash(self)
    }
    fn msvc_hash(&self) -> u64 {
        fnv1a(self.as_bytes())
    }
}

fn gnu_std_hash<K: StdHash>(k: &K) -> u64 {
    k.gnu_hash()
}

fn msvc_std_hash<K: StdHash>(k: &K) -> u64 {
    k.msvc_hash()
}

// ---------------------------------------------------------------------------------------------
// Microsoft STL `std::unordered_set` (<xhash>): a doubly linked list of all elements plus a
// power-of-two bucket vector holding each bucket's first and last element (inclusive range).
// ---------------------------------------------------------------------------------------------

const HEAD: usize = 0;

#[derive(Clone, Debug)]
struct MNode<K> {
    key: Option<K>,
    prev: usize,
    next: usize,
}

/// Microsoft's `std::unordered_set<K>` (VS 2022) with a given hash function.
#[derive(Clone, Debug)]
pub struct MsvcSet<K: Clone + PartialEq> {
    nodes: Vec<MNode<K>>,
    free: Vec<usize>,
    /// (lo, hi) per bucket, `HEAD` when empty.
    buckets: Vec<(usize, usize)>,
    mask: u64,
    len: usize,
    hash: fn(&K) -> u64,
}

const MIN_BUCKETS: usize = 8;

impl<K: Clone + PartialEq> MsvcSet<K> {
    pub fn new(hash: fn(&K) -> u64) -> Self {
        MsvcSet { nodes: vec![MNode { key: None, prev: HEAD, next: HEAD }], free: Vec::new(), buckets: vec![(HEAD, HEAD); MIN_BUCKETS], mask: MIN_BUCKETS as u64 - 1, len: 0, hash }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn bucket_count(&self) -> usize {
        self.buckets.len()
    }

    fn key(&self, n: usize) -> &K {
        self.nodes[n].key.as_ref().unwrap()
    }

    /// `_Find_last`: (insert before, duplicate).
    fn find_last(&self, k: &K, hashval: u64) -> (usize, Option<usize>) {
        let bucket = (hashval & self.mask) as usize;
        let mut wh = self.buckets[bucket].1;
        if wh == HEAD {
            return (HEAD, None);
        }
        let lo = self.buckets[bucket].0;
        loop {
            if self.key(wh) == k {
                return (self.nodes[wh].next, Some(wh));
            }
            if wh == lo {
                return (wh, None);
            }
            wh = self.nodes[wh].prev;
        }
    }

    pub fn contains(&self, k: &K) -> bool {
        self.find_last(k, (self.hash)(k)).1.is_some()
    }

    /// `_Insert_new_node_before`.
    fn insert_new_node_before(&mut self, hashval: u64, before: usize, node: usize) {
        let after = self.nodes[before].prev;
        self.len += 1;
        self.nodes[node].next = before;
        self.nodes[node].prev = after;
        self.nodes[after].next = node;
        self.nodes[before].prev = node;
        let bucket = (hashval & self.mask) as usize;
        let (lo, hi) = self.buckets[bucket];
        if lo == HEAD {
            self.buckets[bucket] = (node, node);
        } else if lo == before {
            self.buckets[bucket].0 = node;
        } else if hi == after {
            self.buckets[bucket].1 = node;
        }
    }

    fn check_rehash_required_1(&self) -> bool {
        1.0f32 < (self.len + 1) as f32 / self.buckets.len() as f32
    }

    /// `_Desired_grow_bucket_count`.
    fn desired_grow_bucket_count(&self, for_size: usize) -> usize {
        let old = self.buckets.len();
        let req = MIN_BUCKETS.max((for_size as f32 / 1.0f32).ceil() as usize);
        if old >= req {
            return old;
        }
        if old < 512 && old * 8 >= req {
            return old * 8;
        }
        req
    }

    /// Unlink `n` from the list (keeping the node allocated).
    fn list_unlink(&mut self, n: usize) {
        let (p, x) = (self.nodes[n].prev, self.nodes[n].next);
        self.nodes[p].next = x;
        self.nodes[x].prev = p;
    }

    /// `_Unchecked_splice(before, first, last)` for the single node `first` (`last` = its next).
    fn splice_one(&mut self, before: usize, n: usize) {
        self.list_unlink(n);
        let after = self.nodes[before].prev;
        self.nodes[n].prev = after;
        self.nodes[n].next = before;
        self.nodes[after].next = n;
        self.nodes[before].prev = n;
    }

    /// `_Forced_rehash`.
    fn forced_rehash(&mut self, buckets: usize) {
        let buckets = buckets.next_power_of_two();
        self.buckets = vec![(HEAD, HEAD); buckets];
        self.mask = buckets as u64 - 1;
        let mut inserted = self.nodes[HEAD].next;
        while inserted != HEAD {
            let next_inserted = self.nodes[inserted].next;
            let bucket = ((self.hash)(self.key(inserted)) & self.mask) as usize;
            let (lo, hi) = self.buckets[bucket];
            if lo == HEAD {
                self.buckets[bucket] = (inserted, inserted);
                inserted = next_inserted;
                continue;
            }
            let mut insert_before = hi;
            if self.key(inserted) == self.key(insert_before) {
                // equivalent to the bucket's last element (not possible in a set): goes after it
                insert_before = self.nodes[insert_before].next;
                if insert_before != inserted {
                    self.splice_one(insert_before, inserted);
                }
                self.buckets[bucket].1 = inserted;
                inserted = next_inserted;
                continue;
            }
            loop {
                if self.buckets[bucket].0 == insert_before {
                    // no equivalent keys in the bucket: insert at its beginning
                    self.splice_one(insert_before, inserted);
                    self.buckets[bucket].0 = inserted;
                    break;
                }
                insert_before = self.nodes[insert_before].prev;
                if self.key(inserted) == self.key(insert_before) {
                    insert_before = self.nodes[insert_before].next;
                    self.splice_one(insert_before, inserted);
                    break;
                }
            }
            inserted = next_inserted;
        }
    }

    fn rehash_for_1(&mut self) {
        let b = self.desired_grow_bucket_count(self.len + 1);
        self.forced_rehash(b);
    }

    fn alloc_node(&mut self, key: K) -> usize {
        if let Some(i) = self.free.pop() {
            self.nodes[i] = MNode { key: Some(key), prev: HEAD, next: HEAD };
            i
        } else {
            self.nodes.push(MNode { key: Some(key), prev: HEAD, next: HEAD });
            self.nodes.len() - 1
        }
    }

    /// `emplace` / `insert`: returns whether the key was inserted.
    pub fn insert(&mut self, key: K) -> bool {
        let hashval = (self.hash)(&key);
        let (mut before, dup) = self.find_last(&key, hashval);
        if dup.is_some() {
            return false;
        }
        if self.check_rehash_required_1() {
            self.rehash_for_1();
            before = self.find_last(&key, hashval).0;
        }
        let node = self.alloc_node(key);
        self.insert_new_node_before(hashval, before, node);
        true
    }

    /// `_Erase_bucket`.
    fn erase_bucket(&mut self, n: usize, bucket: usize) {
        let (lo, hi) = self.buckets[bucket];
        if hi == n {
            if lo == n {
                self.buckets[bucket] = (HEAD, HEAD);
            } else {
                self.buckets[bucket].1 = self.nodes[n].prev;
            }
        } else if lo == n {
            self.buckets[bucket].0 = self.nodes[n].next;
        }
    }

    /// `erase(key)`.
    pub fn remove(&mut self, k: &K) -> usize {
        let hashval = (self.hash)(k);
        let Some(target) = self.find_last(k, hashval).1 else { return 0 };
        self.erase_bucket(target, (hashval & self.mask) as usize);
        self.list_unlink(target);
        self.nodes[target].key = None;
        self.free.push(target);
        self.len -= 1;
        1
    }

    /// `merge(src)`.
    pub fn merge(&mut self, src: &mut MsvcSet<K>) {
        let mut first = src.nodes[HEAD].next;
        while first != HEAD {
            let candidate = first;
            first = src.nodes[first].next;
            let key = src.key(candidate).clone();
            let hashval = (self.hash)(&key);
            let (mut before, dup) = self.find_last(&key, hashval);
            if dup.is_some() {
                continue;
            }
            if self.check_rehash_required_1() {
                self.rehash_for_1();
                before = self.find_last(&key, hashval).0;
            }
            let source_bucket = (hashval & src.mask) as usize;
            src.erase_bucket(candidate, source_bucket);
            src.list_unlink(candidate);
            src.nodes[candidate].key = None;
            src.free.push(candidate);
            src.len -= 1;
            let node = self.alloc_node(key);
            self.insert_new_node_before(hashval, before, node);
        }
    }

    pub fn iter(&self) -> MsvcIter<'_, K> {
        MsvcIter { set: self, cur: self.nodes[HEAD].next }
    }
}

pub struct MsvcIter<'a, K: Clone + PartialEq> {
    set: &'a MsvcSet<K>,
    cur: usize,
}

impl<'a, K: Clone + PartialEq> Iterator for MsvcIter<'a, K> {
    type Item = &'a K;
    fn next(&mut self) -> Option<&'a K> {
        if self.cur == HEAD {
            return None;
        }
        let n = &self.set.nodes[self.cur];
        self.cur = n.next;
        n.key.as_ref()
    }
}

// ---------------------------------------------------------------------------------------------

/// `std::unordered_set<K>` of the standard library being emulated ([`crate::flavor`]).
#[derive(Clone, Debug)]
pub enum UnorderedSet<K: Clone + PartialEq> {
    Gnu(GnuSet<K>),
    Msvc(MsvcSet<K>),
}

pub enum Iter<'a, K: Clone + PartialEq> {
    Gnu(GnuIter<'a, K>),
    Msvc(MsvcIter<'a, K>),
}

impl<'a, K: Clone + PartialEq> Iterator for Iter<'a, K> {
    type Item = &'a K;
    fn next(&mut self) -> Option<&'a K> {
        match self {
            Iter::Gnu(i) => i.next(),
            Iter::Msvc(i) => i.next(),
        }
    }
}

impl<K: Clone + PartialEq> UnorderedSet<K> {
    /// A set with a user-defined hash (a `std::hash` specialisation of the program, the same with
    /// both standard libraries).
    pub fn new(hash: fn(&K) -> u64) -> Self {
        if crate::flavor::msvc() {
            UnorderedSet::Msvc(MsvcSet::new(hash))
        } else {
            UnorderedSet::Gnu(GnuSet::new(hash))
        }
    }

    pub fn len(&self) -> usize {
        match self {
            UnorderedSet::Gnu(s) => s.len(),
            UnorderedSet::Msvc(s) => s.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn bucket_count(&self) -> usize {
        match self {
            UnorderedSet::Gnu(s) => s.bucket_count(),
            UnorderedSet::Msvc(s) => s.bucket_count(),
        }
    }

    pub fn contains(&self, k: &K) -> bool {
        match self {
            UnorderedSet::Gnu(s) => s.contains(k),
            UnorderedSet::Msvc(s) => s.contains(k),
        }
    }

    pub fn insert(&mut self, key: K) -> bool {
        match self {
            UnorderedSet::Gnu(s) => s.insert(key),
            UnorderedSet::Msvc(s) => s.insert(key),
        }
    }

    pub fn remove(&mut self, k: &K) -> usize {
        match self {
            UnorderedSet::Gnu(s) => s.remove(k),
            UnorderedSet::Msvc(s) => s.remove(k),
        }
    }

    pub fn merge(&mut self, src: &mut UnorderedSet<K>) {
        match (self, src) {
            (UnorderedSet::Gnu(a), UnorderedSet::Gnu(b)) => a.merge(b),
            (UnorderedSet::Msvc(a), UnorderedSet::Msvc(b)) => a.merge(b),
            _ => panic!("merging unordered sets of different flavors"),
        }
    }

    pub fn iter(&self) -> Iter<'_, K> {
        match self {
            UnorderedSet::Gnu(s) => Iter::Gnu(s.iter()),
            UnorderedSet::Msvc(s) => Iter::Msvc(s.iter()),
        }
    }

    pub fn to_vec(&self) -> Vec<K> {
        self.iter().cloned().collect()
    }
}

impl<K: Clone + PartialEq + StdHash> UnorderedSet<K> {
    /// A set using `std::hash<K>` of the emulated standard library.
    pub fn new_std() -> Self {
        if crate::flavor::msvc() {
            UnorderedSet::Msvc(MsvcSet::new(msvc_std_hash::<K>))
        } else {
            UnorderedSet::Gnu(GnuSet::new(gnu_std_hash::<K>))
        }
    }
}
