//! `std::map` / `std::set` as implemented by libstdc++ (`_Rb_tree`), including the exact
//! rebalancing, hinted insertion and erase algorithms.
//!
//! The tree shape only matters when the comparator is not a strict weak ordering over the whole
//! container (as is the case for the beach line of Boost.Polygon's Voronoi builder, whose
//! comparator is only meaningful relative to the current sweep line position). Reproducing the
//! exact node layout guarantees that every comparison libstdc++ performs is performed here too,
//! in the same order and on the same operands.
//!
//! Nodes live in an arena and are addressed by index ("iterators"). Erased nodes are not
//! reused, so stale iterators never alias new elements. Index 0 is the header (`end()`).

const RED: bool = false;
const BLACK: bool = true;
const NIL: usize = usize::MAX;

/// The header node index, which is also the `end()` iterator.
pub const END: usize = 0;

struct Node<K, V> {
    color: bool,
    parent: usize,
    left: usize,
    right: usize,
    kv: Option<(K, V)>,
}

pub struct RbMap<K, V> {
    nodes: Vec<Node<K, V>>,
    count: usize,
}

impl<K, V> Default for RbMap<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V> RbMap<K, V> {
    pub fn new() -> Self {
        RbMap { nodes: vec![Node { color: RED, parent: NIL, left: END, right: END, kv: None }], count: 0 }
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn clear(&mut self) {
        self.nodes.truncate(1);
        self.nodes[0] = Node { color: RED, parent: NIL, left: END, right: END, kv: None };
        self.count = 0;
    }

    #[inline]
    pub fn begin(&self) -> usize {
        self.nodes[END].left
    }

    #[inline]
    pub fn end(&self) -> usize {
        END
    }

    #[inline]
    fn root(&self) -> usize {
        self.nodes[END].parent
    }

    #[inline]
    pub fn key(&self, it: usize) -> &K {
        &self.nodes[it].kv.as_ref().expect("dereferenced end() or erased iterator").0
    }

    /// Mutable access to a key (the `const_cast` idiom). The caller is responsible for keeping
    /// the ordering intact.
    #[inline]
    pub fn key_mut(&mut self, it: usize) -> &mut K {
        &mut self.nodes[it].kv.as_mut().expect("dereferenced end() or erased iterator").0
    }

    #[inline]
    pub fn value(&self, it: usize) -> &V {
        &self.nodes[it].kv.as_ref().expect("dereferenced end() or erased iterator").1
    }

    #[inline]
    pub fn value_mut(&mut self, it: usize) -> &mut V {
        &mut self.nodes[it].kv.as_mut().expect("dereferenced end() or erased iterator").1
    }

    /// `++it` (`_Rb_tree_increment`).
    pub fn next(&self, mut x: usize) -> usize {
        let n = &self.nodes;
        if n[x].right != NIL {
            x = n[x].right;
            while n[x].left != NIL {
                x = n[x].left;
            }
        } else {
            let mut y = n[x].parent;
            while x == n[y].right {
                x = y;
                y = n[y].parent;
            }
            if n[x].right != y {
                x = y;
            }
        }
        x
    }

    /// `--it` (`_Rb_tree_decrement`).
    pub fn prev(&self, mut x: usize) -> usize {
        let n = &self.nodes;
        if n[x].color == RED && n[x].parent != NIL && n[n[x].parent].parent == x {
            x = n[x].right;
        } else if n[x].left != NIL {
            let mut y = n[x].left;
            while n[y].right != NIL {
                y = n[y].right;
            }
            x = y;
        } else {
            let mut y = n[x].parent;
            while x == n[y].left {
                x = y;
                y = n[y].parent;
            }
            x = y;
        }
        x
    }

    fn rotate_left(&mut self, x: usize) {
        let y = self.nodes[x].right;
        self.nodes[x].right = self.nodes[y].left;
        if self.nodes[y].left != NIL {
            let yl = self.nodes[y].left;
            self.nodes[yl].parent = x;
        }
        self.nodes[y].parent = self.nodes[x].parent;
        let xp = self.nodes[x].parent;
        if x == self.root() {
            self.nodes[END].parent = y;
        } else if x == self.nodes[xp].left {
            self.nodes[xp].left = y;
        } else {
            self.nodes[xp].right = y;
        }
        self.nodes[y].left = x;
        self.nodes[x].parent = y;
    }

    fn rotate_right(&mut self, x: usize) {
        let y = self.nodes[x].left;
        self.nodes[x].left = self.nodes[y].right;
        if self.nodes[y].right != NIL {
            let yr = self.nodes[y].right;
            self.nodes[yr].parent = x;
        }
        self.nodes[y].parent = self.nodes[x].parent;
        let xp = self.nodes[x].parent;
        if x == self.root() {
            self.nodes[END].parent = y;
        } else if x == self.nodes[xp].right {
            self.nodes[xp].right = y;
        } else {
            self.nodes[xp].left = y;
        }
        self.nodes[y].right = x;
        self.nodes[x].parent = y;
    }

    /// `_Rb_tree_insert_and_rebalance`.
    fn insert_and_rebalance(&mut self, insert_left: bool, mut x: usize, p: usize) {
        self.nodes[x].parent = p;
        self.nodes[x].left = NIL;
        self.nodes[x].right = NIL;
        self.nodes[x].color = RED;

        if insert_left {
            self.nodes[p].left = x; // also makes leftmost = x when p == header
            if p == END {
                self.nodes[END].parent = x;
                self.nodes[END].right = x;
            } else if p == self.nodes[END].left {
                self.nodes[END].left = x;
            }
        } else {
            self.nodes[p].right = x;
            if p == self.nodes[END].right {
                self.nodes[END].right = x;
            }
        }

        while x != self.root() && self.nodes[self.nodes[x].parent].color == RED {
            let xp = self.nodes[x].parent;
            let xpp = self.nodes[xp].parent;
            if xp == self.nodes[xpp].left {
                let y = self.nodes[xpp].right;
                if y != NIL && self.nodes[y].color == RED {
                    self.nodes[xp].color = BLACK;
                    self.nodes[y].color = BLACK;
                    self.nodes[xpp].color = RED;
                    x = xpp;
                } else {
                    if x == self.nodes[xp].right {
                        x = xp;
                        self.rotate_left(x);
                    }
                    let xp = self.nodes[x].parent;
                    self.nodes[xp].color = BLACK;
                    self.nodes[xpp].color = RED;
                    self.rotate_right(xpp);
                }
            } else {
                let y = self.nodes[xpp].left;
                if y != NIL && self.nodes[y].color == RED {
                    self.nodes[xp].color = BLACK;
                    self.nodes[y].color = BLACK;
                    self.nodes[xpp].color = RED;
                    x = xpp;
                } else {
                    if x == self.nodes[xp].left {
                        x = xp;
                        self.rotate_right(x);
                    }
                    let xp = self.nodes[x].parent;
                    self.nodes[xp].color = BLACK;
                    self.nodes[xpp].color = RED;
                    self.rotate_left(xpp);
                }
            }
        }
        let r = self.root();
        self.nodes[r].color = BLACK;
    }

    fn minimum(&self, mut x: usize) -> usize {
        while self.nodes[x].left != NIL {
            x = self.nodes[x].left;
        }
        x
    }

    fn maximum(&self, mut x: usize) -> usize {
        while self.nodes[x].right != NIL {
            x = self.nodes[x].right;
        }
        x
    }

    /// `_Rb_tree_rebalance_for_erase`; returns the unlinked node.
    fn rebalance_for_erase(&mut self, z: usize) -> usize {
        let mut y = z;
        let mut x: usize;
        let mut x_parent: usize;

        if self.nodes[y].left == NIL {
            x = self.nodes[y].right;
        } else if self.nodes[y].right == NIL {
            x = self.nodes[y].left;
        } else {
            y = self.nodes[y].right;
            while self.nodes[y].left != NIL {
                y = self.nodes[y].left;
            }
            x = self.nodes[y].right;
        }
        if y != z {
            // relink y in place of z
            let zl = self.nodes[z].left;
            self.nodes[zl].parent = y;
            self.nodes[y].left = zl;
            if y != self.nodes[z].right {
                x_parent = self.nodes[y].parent;
                if x != NIL {
                    self.nodes[x].parent = self.nodes[y].parent;
                }
                let yp = self.nodes[y].parent;
                self.nodes[yp].left = x;
                let zr = self.nodes[z].right;
                self.nodes[y].right = zr;
                self.nodes[zr].parent = y;
            } else {
                x_parent = y;
            }
            let zp = self.nodes[z].parent;
            if self.root() == z {
                self.nodes[END].parent = y;
            } else if self.nodes[zp].left == z {
                self.nodes[zp].left = y;
            } else {
                self.nodes[zp].right = y;
            }
            self.nodes[y].parent = zp;
            let c = self.nodes[y].color;
            self.nodes[y].color = self.nodes[z].color;
            self.nodes[z].color = c;
            y = z;
        } else {
            x_parent = self.nodes[y].parent;
            if x != NIL {
                self.nodes[x].parent = self.nodes[y].parent;
            }
            let zp = self.nodes[z].parent;
            if self.root() == z {
                self.nodes[END].parent = x;
            } else if self.nodes[zp].left == z {
                self.nodes[zp].left = x;
            } else {
                self.nodes[zp].right = x;
            }
            if self.nodes[END].left == z {
                if self.nodes[z].right == NIL {
                    self.nodes[END].left = zp;
                } else {
                    self.nodes[END].left = self.minimum(x);
                }
            }
            if self.nodes[END].right == z {
                if self.nodes[z].left == NIL {
                    self.nodes[END].right = zp;
                } else {
                    self.nodes[END].right = self.maximum(x);
                }
            }
        }
        if self.nodes[y].color != RED {
            while x != self.root() && (x == NIL || self.nodes[x].color == BLACK) {
                if x == self.nodes[x_parent].left {
                    let mut w = self.nodes[x_parent].right;
                    if self.nodes[w].color == RED {
                        self.nodes[w].color = BLACK;
                        self.nodes[x_parent].color = RED;
                        self.rotate_left(x_parent);
                        w = self.nodes[x_parent].right;
                    }
                    let wl = self.nodes[w].left;
                    let wr = self.nodes[w].right;
                    if (wl == NIL || self.nodes[wl].color == BLACK) && (wr == NIL || self.nodes[wr].color == BLACK) {
                        self.nodes[w].color = RED;
                        x = x_parent;
                        x_parent = self.nodes[x_parent].parent;
                    } else {
                        if wr == NIL || self.nodes[wr].color == BLACK {
                            self.nodes[wl].color = BLACK;
                            self.nodes[w].color = RED;
                            self.rotate_right(w);
                            w = self.nodes[x_parent].right;
                        }
                        self.nodes[w].color = self.nodes[x_parent].color;
                        self.nodes[x_parent].color = BLACK;
                        let wr = self.nodes[w].right;
                        if wr != NIL {
                            self.nodes[wr].color = BLACK;
                        }
                        self.rotate_left(x_parent);
                        break;
                    }
                } else {
                    let mut w = self.nodes[x_parent].left;
                    if self.nodes[w].color == RED {
                        self.nodes[w].color = BLACK;
                        self.nodes[x_parent].color = RED;
                        self.rotate_right(x_parent);
                        w = self.nodes[x_parent].left;
                    }
                    let wl = self.nodes[w].left;
                    let wr = self.nodes[w].right;
                    if (wr == NIL || self.nodes[wr].color == BLACK) && (wl == NIL || self.nodes[wl].color == BLACK) {
                        self.nodes[w].color = RED;
                        x = x_parent;
                        x_parent = self.nodes[x_parent].parent;
                    } else {
                        if wl == NIL || self.nodes[wl].color == BLACK {
                            self.nodes[wr].color = BLACK;
                            self.nodes[w].color = RED;
                            self.rotate_left(w);
                            w = self.nodes[x_parent].left;
                        }
                        self.nodes[w].color = self.nodes[x_parent].color;
                        self.nodes[x_parent].color = BLACK;
                        let wl = self.nodes[w].left;
                        if wl != NIL {
                            self.nodes[wl].color = BLACK;
                        }
                        self.rotate_right(x_parent);
                        break;
                    }
                }
            }
            if x != NIL {
                self.nodes[x].color = BLACK;
            }
        }
        y
    }

    /// `erase(iterator)`.
    pub fn erase(&mut self, it: usize) {
        let y = self.rebalance_for_erase(it);
        self.nodes[y].kv = None;
        self.nodes[y].parent = NIL;
        self.nodes[y].left = NIL;
        self.nodes[y].right = NIL;
        self.count -= 1;
    }

    fn create_node(&mut self, k: K, v: V) -> usize {
        self.nodes.push(Node { color: RED, parent: NIL, left: NIL, right: NIL, kv: Some((k, v)) });
        self.nodes.len() - 1
    }

    /// `lower_bound(k)`.
    pub fn lower_bound<F: FnMut(&K, &K) -> bool>(&self, k: &K, mut less: F) -> usize {
        let mut x = self.root();
        let mut y = END;
        while x != NIL {
            if !less(self.key(x), k) {
                y = x;
                x = self.nodes[x].left;
            } else {
                x = self.nodes[x].right;
            }
        }
        y
    }

    /// `upper_bound(k)`.
    pub fn upper_bound<F: FnMut(&K, &K) -> bool>(&self, k: &K, mut less: F) -> usize {
        let mut x = self.root();
        let mut y = END;
        while x != NIL {
            if less(k, self.key(x)) {
                y = x;
                x = self.nodes[x].left;
            } else {
                x = self.nodes[x].right;
            }
        }
        y
    }

    /// `find(k)`.
    pub fn find<F: FnMut(&K, &K) -> bool>(&self, k: &K, mut less: F) -> usize {
        let j = self.lower_bound(k, &mut less);
        if j == END || less(k, self.key(j)) {
            END
        } else {
            j
        }
    }

    /// `_M_get_insert_unique_pos`: (x, p) with p == NIL meaning "equivalent key at x".
    fn get_insert_unique_pos<F: FnMut(&K, &K) -> bool>(&self, k: &K, less: &mut F) -> (usize, usize) {
        let mut x = self.root();
        let mut y = END;
        let mut comp = true;
        while x != NIL {
            y = x;
            comp = less(k, self.key(x));
            x = if comp { self.nodes[x].left } else { self.nodes[x].right };
        }
        let mut j = y;
        if comp {
            if j == self.begin() {
                return (x, y);
            } else {
                j = self.prev(j);
            }
        }
        if less(self.key(j), k) {
            return (x, y);
        }
        (j, NIL)
    }

    /// `_M_get_insert_hint_unique_pos`.
    fn get_insert_hint_unique_pos<F: FnMut(&K, &K) -> bool>(&self, pos: usize, k: &K, less: &mut F) -> (usize, usize) {
        if pos == END {
            if self.count > 0 && less(self.key(self.nodes[END].right), k) {
                (NIL, self.nodes[END].right)
            } else {
                self.get_insert_unique_pos(k, less)
            }
        } else if less(k, self.key(pos)) {
            if pos == self.nodes[END].left {
                (self.nodes[END].left, self.nodes[END].left)
            } else {
                let before = self.prev(pos);
                if less(self.key(before), k) {
                    if self.nodes[before].right == NIL {
                        (NIL, before)
                    } else {
                        (pos, pos)
                    }
                } else {
                    self.get_insert_unique_pos(k, less)
                }
            }
        } else if less(self.key(pos), k) {
            if pos == self.nodes[END].right {
                (NIL, self.nodes[END].right)
            } else {
                let after = self.next(pos);
                if less(k, self.key(after)) {
                    if self.nodes[pos].right == NIL {
                        (NIL, pos)
                    } else {
                        (after, after)
                    }
                } else {
                    self.get_insert_unique_pos(k, less)
                }
            }
        } else {
            (pos, NIL)
        }
    }

    /// `_M_insert_(x, p, v)`.
    fn insert_at<F: FnMut(&K, &K) -> bool>(&mut self, x: usize, p: usize, k: K, v: V, less: &mut F) -> usize {
        let insert_left = x != NIL || p == END || less(&k, self.key(p));
        let z = self.create_node(k, v);
        self.insert_and_rebalance(insert_left, z, p);
        self.count += 1;
        z
    }

    /// `insert(hint, value)` / `emplace_hint`: returns the iterator to the inserted element, or
    /// to the equivalent element that prevented the insertion.
    pub fn insert_hint<F: FnMut(&K, &K) -> bool>(&mut self, pos: usize, k: K, v: V, mut less: F) -> usize {
        let (x, p) = self.get_insert_hint_unique_pos(pos, &k, &mut less);
        if p != NIL {
            self.insert_at(x, p, k, v, &mut less)
        } else {
            x
        }
    }

    /// `insert(value)`: returns (iterator, inserted).
    pub fn insert<F: FnMut(&K, &K) -> bool>(&mut self, k: K, v: V, mut less: F) -> (usize, bool) {
        let (x, p) = self.get_insert_unique_pos(&k, &mut less);
        if p != NIL {
            (self.insert_at(x, p, k, v, &mut less), true)
        } else {
            (x, false)
        }
    }

    /// In-order iteration over all live elements.
    pub fn iter(&self) -> RbIter<'_, K, V> {
        RbIter { map: self, it: self.begin() }
    }
}

pub struct RbIter<'a, K, V> {
    map: &'a RbMap<K, V>,
    it: usize,
}

impl<'a, K, V> Iterator for RbIter<'a, K, V> {
    type Item = (usize, &'a K, &'a V);
    fn next(&mut self) -> Option<Self::Item> {
        if self.it == END {
            return None;
        }
        let it = self.it;
        self.it = self.map.next(it);
        let (k, v) = self.map.nodes[it].kv.as_ref().unwrap();
        Some((it, k, v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordered_inserts_and_erases() {
        let mut m: RbMap<i32, i32> = RbMap::new();
        let less = |a: &i32, b: &i32| a < b;
        let mut x: u64 = 12345;
        let mut live = std::collections::BTreeSet::new();
        let mut its = std::collections::HashMap::new();
        for _ in 0..5000 {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let k = ((x >> 33) % 500) as i32;
            if live.contains(&k) {
                m.erase(its[&k]);
                live.remove(&k);
            } else {
                let hint = m.lower_bound(&(k + 3), less);
                let it = m.insert_hint(hint, k, k * 2, less);
                its.insert(k, it);
                live.insert(k);
            }
            let got: Vec<i32> = m.iter().map(|(_, k, _)| *k).collect();
            let want: Vec<i32> = live.iter().copied().collect();
            assert_eq!(got, want);
            if let Some(&last) = live.iter().next_back() {
                assert_eq!(*m.key(m.prev(m.end())), last);
            }
        }
    }
}

impl<K: std::fmt::Debug, V> RbMap<K, V> {
    /// Pre-order dump of the tree shape (for differential tests against libstdc++).
    pub fn debug_structure(&self) -> String {
        let mut out = String::new();
        self.dump(self.root(), &mut out);
        out
    }

    fn dump(&self, x: usize, out: &mut String) {
        if x == NIL {
            out.push('.');
            return;
        }
        out.push_str(&format!("({:?}{}", self.key(x), if self.nodes[x].color == RED { 'r' } else { 'b' }));
        self.dump(self.nodes[x].left, out);
        self.dump(self.nodes[x].right, out);
        out.push(')');
    }
}
