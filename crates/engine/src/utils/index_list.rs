//! An arena-backed doubly linked list with stable indices, standing in for `std::list` where
//! CuraEngine keeps pointers/iterators into the list (the half-edge graph of Arachne, transition
//! lists). Erased entries are unlinked but keep their storage, so stale indices never alias.

pub const NIL: usize = usize::MAX;

#[derive(Clone, Debug)]
struct Entry<T> {
    val: T,
    prev: usize,
    next: usize,
    alive: bool,
}

#[derive(Clone, Debug)]
pub struct IndexList<T> {
    entries: Vec<Entry<T>>,
    head: usize,
    tail: usize,
    len: usize,
}

impl<T> Default for IndexList<T> {
    fn default() -> Self {
        IndexList { entries: Vec::new(), head: NIL, tail: NIL, len: 0 }
    }
}

impl<T> IndexList<T> {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// First element (`begin()`), or `NIL`.
    #[inline]
    pub fn first(&self) -> usize {
        self.head
    }

    /// Last element, or `NIL`.
    #[inline]
    pub fn last(&self) -> usize {
        self.tail
    }

    #[inline]
    pub fn next(&self, i: usize) -> usize {
        self.entries[i].next
    }

    #[inline]
    pub fn prev(&self, i: usize) -> usize {
        self.entries[i].prev
    }

    #[inline]
    pub fn is_alive(&self, i: usize) -> bool {
        i != NIL && self.entries[i].alive
    }

    pub fn push_back(&mut self, val: T) -> usize {
        let idx = self.entries.len();
        self.entries.push(Entry { val, prev: self.tail, next: NIL, alive: true });
        if self.tail != NIL {
            self.entries[self.tail].next = idx;
        } else {
            self.head = idx;
        }
        self.tail = idx;
        self.len += 1;
        idx
    }

    pub fn push_front(&mut self, val: T) -> usize {
        let idx = self.entries.len();
        self.entries.push(Entry { val, prev: NIL, next: self.head, alive: true });
        if self.head != NIL {
            self.entries[self.head].prev = idx;
        } else {
            self.tail = idx;
        }
        self.head = idx;
        self.len += 1;
        idx
    }

    /// `erase(it)`: returns the index of the following element (or `NIL`).
    pub fn erase(&mut self, i: usize) -> usize {
        debug_assert!(self.entries[i].alive);
        let (p, n) = (self.entries[i].prev, self.entries[i].next);
        if p != NIL {
            self.entries[p].next = n;
        } else {
            self.head = n;
        }
        if n != NIL {
            self.entries[n].prev = p;
        } else {
            self.tail = p;
        }
        self.entries[i].alive = false;
        self.entries[i].prev = NIL;
        self.entries[i].next = NIL;
        self.len -= 1;
        n
    }

    pub fn pop_front(&mut self) {
        if self.head != NIL {
            self.erase(self.head);
        }
    }

    pub fn pop_back(&mut self) {
        if self.tail != NIL {
            self.erase(self.tail);
        }
    }

    pub fn clear(&mut self) {
        let mut i = self.head;
        while i != NIL {
            let n = self.entries[i].next;
            self.entries[i].alive = false;
            self.entries[i].prev = NIL;
            self.entries[i].next = NIL;
            i = n;
        }
        self.head = NIL;
        self.tail = NIL;
        self.len = 0;
    }

    pub fn front(&self) -> &T {
        &self.entries[self.head].val
    }

    pub fn back(&self) -> &T {
        &self.entries[self.tail].val
    }

    /// Indices of the live elements in list order.
    pub fn indices(&self) -> Vec<usize> {
        let mut out = Vec::with_capacity(self.len);
        let mut i = self.head;
        while i != NIL {
            out.push(i);
            i = self.entries[i].next;
        }
        out
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> + '_ {
        let mut i = self.head;
        std::iter::from_fn(move || {
            if i == NIL {
                return None;
            }
            let r = &self.entries[i].val;
            i = self.entries[i].next;
            Some(r)
        })
    }

    /// `std::list::sort(comp)`: a stable sort; the indices of the elements are preserved.
    pub fn stable_sort_by<F: FnMut(&T, &T) -> bool>(&mut self, mut less: F) {
        let mut idx = self.indices();
        // Rust's sort_by is stable; derive an Ordering from the strict weak ordering.
        idx.sort_by(|&a, &b| {
            if less(&self.entries[a].val, &self.entries[b].val) {
                std::cmp::Ordering::Less
            } else if less(&self.entries[b].val, &self.entries[a].val) {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        });
        let mut prev = NIL;
        for &i in &idx {
            self.entries[i].prev = prev;
            if prev != NIL {
                self.entries[prev].next = i;
            }
            prev = i;
        }
        if let Some(&last) = idx.last() {
            self.entries[last].next = NIL;
            self.head = idx[0];
            self.tail = last;
        }
    }
}

impl<T> std::ops::Index<usize> for IndexList<T> {
    type Output = T;
    #[inline]
    fn index(&self, i: usize) -> &T {
        &self.entries[i].val
    }
}

impl<T> std::ops::IndexMut<usize> for IndexList<T> {
    #[inline]
    fn index_mut(&mut self, i: usize) -> &mut T {
        &mut self.entries[i].val
    }
}
