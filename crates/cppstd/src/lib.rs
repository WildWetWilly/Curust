//! Deterministic re-implementations of the libstdc++ (GCC) standard algorithms that
//! CuraEngine relies on.
//!
//! CuraEngine sorts with `std::sort`, which is *not* stable: elements that compare equal
//! end up in an order that is determined by the exact introsort implementation. Because the
//! order of equal elements frequently leaks into the generated toolpaths (which polygon is
//! printed first, where a seam starts, ...), a faithful port needs the exact same algorithm.
//! This crate mirrors libstdc++'s `std::sort`, heap algorithms, `std::priority_queue`,
//! `std::nth_element` and the "first element wins" semantics of `std::min_element` /
//! `std::max_element`.
//!
//! All functions take a strict-weak-ordering "less" predicate, like the C++ originals.

pub mod flavor;
pub mod msvc;
pub mod rbtree;
pub mod unordered;
pub mod boost_foa;
pub mod boost_rtree;

use std::ptr;

const S_THRESHOLD: usize = 16;

#[inline]
fn lg(n: usize) -> usize {
    (usize::BITS - 1 - n.leading_zeros()) as usize
}

/// `std::sort(first, last, comp)` exactly as implemented by libstdc++ (introsort + final
/// insertion sort), or by Microsoft's STL when emulating the Windows build ([`flavor`]).
pub fn sort_by<T, F>(v: &mut [T], mut less: F)
where
    F: FnMut(&T, &T) -> bool,
{
    let n = v.len();
    if n < 2 {
        return;
    }
    if flavor::msvc() {
        msvc::sort_by(v, &mut less);
        return;
    }
    introsort_loop(v, 0, n, lg(n) * 2, &mut less);
    final_insertion_sort(v, 0, n, &mut less);
}

/// `std::sort` using `operator<`.
pub fn sort<T: PartialOrd>(v: &mut [T]) {
    sort_by(v, |a, b| a < b);
}

/// `std::sort` using a key and `operator<` on the key.
pub fn sort_by_key<T, K: PartialOrd, F: FnMut(&T) -> K>(v: &mut [T], mut key: F) {
    sort_by(v, |a, b| key(a) < key(b));
}

/// `std::list<T>::sort(comp)` exactly as implemented by libstdc++ (bottom-up merge sort with 64
/// bins), so that the result is identical even for comparators that are not strict weak orderings.
pub fn list_sort_by<T, F>(v: Vec<T>, mut less: F) -> Vec<T>
where
    F: FnMut(&T, &T) -> bool,
{
    if v.len() < 2 {
        return v;
    }
    if flavor::msvc() {
        let perm = msvc::list_sort_perm(&v, &mut less);
        let mut slots: Vec<Option<T>> = v.into_iter().map(Some).collect();
        return perm.into_iter().map(|i| slots[i].take().unwrap()).collect();
    }
    // `this->merge(x)`: elements of `x` are inserted before the current element of `this` when
    // `comp(x_elem, this_elem)`.
    fn merge<T, F: FnMut(&T, &T) -> bool>(this: Vec<T>, x: Vec<T>, less: &mut F) -> Vec<T> {
        if x.is_empty() {
            return this;
        }
        let mut out = Vec::with_capacity(this.len() + x.len());
        let mut a = this.into_iter().peekable();
        let mut b = x.into_iter().peekable();
        while let (Some(first1), Some(first2)) = (a.peek(), b.peek()) {
            if less(first2, first1) {
                out.push(b.next().unwrap());
            } else {
                out.push(a.next().unwrap());
            }
        }
        out.extend(a);
        out.extend(b);
        out
    }
    let mut input = std::collections::VecDeque::from(v);
    let mut tmp: Vec<Vec<T>> = (0..64).map(|_| Vec::new()).collect();
    let mut fill = 0usize;
    loop {
        let mut carry = vec![input.pop_front().unwrap()];
        let mut counter = 0usize;
        while counter != fill && !tmp[counter].is_empty() {
            let bin = std::mem::take(&mut tmp[counter]);
            carry = merge(bin, carry, &mut less);
            counter += 1;
        }
        std::mem::swap(&mut carry, &mut tmp[counter]);
        if counter == fill {
            fill += 1;
        }
        if input.is_empty() {
            break;
        }
    }
    for counter in 1..fill {
        let older = std::mem::take(&mut tmp[counter]);
        let newer = std::mem::take(&mut tmp[counter - 1]);
        tmp[counter] = merge(older, newer, &mut less);
    }
    std::mem::take(&mut tmp[fill - 1])
}

/// `std::stable_sort`: an exact port of libstdc++'s buffered merge sort (GCC 13), so that the
/// result is identical even for comparators that are not strict weak orderings.
pub fn stable_sort_by<T, F>(v: &mut [T], mut less: F)
where
    F: FnMut(&T, &T) -> bool,
{
    let len = v.len();
    if len == 0 {
        return;
    }
    if flavor::msvc() {
        let perm = msvc::stable_sort_perm(v, &mut less);
        apply_permutation(v, &perm);
        return;
    }
    let mut idx: Vec<usize> = (0..len).collect();
    {
        let mut cmp = |a: usize, b: usize| less(&v[a], &v[b]);
        let buf_size = (len + 1) / 2;
        let mut buffer = vec![0usize; buf_size];
        stable_sort_adaptive(&mut idx, 0, buf_size, len, &mut buffer, &mut cmp);
    }
    apply_permutation(v, &idx);
}

/// Reorder `v` so that `v_new[k] = v_old[perm[k]]`.
fn apply_permutation<T>(v: &mut [T], perm: &[usize]) {
    let n = v.len();
    let mut done = vec![false; n];
    for start in 0..n {
        if done[start] {
            continue;
        }
        // Follow the cycle: position k receives the element from perm[k].
        let mut k = start;
        loop {
            done[k] = true;
            let src = perm[k];
            if src == start {
                break;
            }
            v.swap(k, src);
            k = src;
        }
    }
}

fn idx_insertion_sort<F: FnMut(usize, usize) -> bool>(a: &mut [usize], first: usize, last: usize, comp: &mut F) {
    if first == last {
        return;
    }
    for i in first + 1..last {
        let val = a[i];
        if comp(val, a[first]) {
            // move_backward(first, i, i + 1); *first = val;
            a.copy_within(first..i, first + 1);
            a[first] = val;
        } else {
            // __unguarded_linear_insert
            let mut last_pos = i;
            let mut next = i - 1;
            while comp(val, a[next]) {
                a[last_pos] = a[next];
                last_pos = next;
                next -= 1;
            }
            a[last_pos] = val;
        }
    }
}

fn idx_chunk_insertion_sort<F: FnMut(usize, usize) -> bool>(a: &mut [usize], mut first: usize, last: usize, chunk_size: usize, comp: &mut F) {
    while last - first >= chunk_size {
        idx_insertion_sort(a, first, first + chunk_size, comp);
        first += chunk_size;
    }
    idx_insertion_sort(a, first, last, comp);
}

/// `std::__move_merge` from `src[first1..last1]` and `src[first2..last2]` into `dst[result..]`.
fn idx_move_merge<F: FnMut(usize, usize) -> bool>(
    src: &[usize],
    mut first1: usize,
    last1: usize,
    mut first2: usize,
    last2: usize,
    dst: &mut [usize],
    mut result: usize,
    comp: &mut F,
) -> usize {
    while first1 != last1 && first2 != last2 {
        if comp(src[first2], src[first1]) {
            dst[result] = src[first2];
            first2 += 1;
        } else {
            dst[result] = src[first1];
            first1 += 1;
        }
        result += 1;
    }
    for i in first1..last1 {
        dst[result] = src[i];
        result += 1;
    }
    for i in first2..last2 {
        dst[result] = src[i];
        result += 1;
    }
    result
}

fn idx_merge_sort_loop<F: FnMut(usize, usize) -> bool>(src: &[usize], mut first: usize, last: usize, dst: &mut [usize], mut result: usize, step_size: usize, comp: &mut F) {
    let two_step = 2 * step_size;
    while last - first >= two_step {
        result = idx_move_merge(src, first, first + step_size, first + step_size, first + two_step, dst, result, comp);
        first += two_step;
    }
    let step = (last - first).min(step_size);
    idx_move_merge(src, first, first + step, first + step, last, dst, result, comp);
}

fn idx_merge_sort_with_buffer<F: FnMut(usize, usize) -> bool>(a: &mut [usize], first: usize, last: usize, buffer: &mut [usize], comp: &mut F) {
    let len = last - first;
    let mut step_size = 7usize; // _S_chunk_size
    idx_chunk_insertion_sort(a, first, last, step_size, comp);
    while step_size < len {
        // merge from a[first..last] into buffer[0..len]
        let src: Vec<usize> = a[first..last].to_vec();
        idx_merge_sort_loop(&src, 0, len, buffer, 0, step_size, comp);
        step_size *= 2;
        let bsrc: Vec<usize> = buffer[0..len].to_vec();
        idx_merge_sort_loop(&bsrc, 0, len, &mut a[first..last], 0, step_size, comp);
        step_size *= 2;
    }
}

fn idx_merge_adaptive<F: FnMut(usize, usize) -> bool>(a: &mut [usize], first: usize, middle: usize, last: usize, len1: usize, len2: usize, buffer: &mut [usize], comp: &mut F) {
    if len1 <= len2 {
        // buffer_end = move(first, middle, buffer)
        buffer[..len1].copy_from_slice(&a[first..middle]);
        // __move_merge_adaptive(buffer, buffer_end, middle, last, first, comp)
        let mut first1 = 0usize;
        let last1 = len1;
        let mut first2 = middle;
        let last2 = last;
        let mut result = first;
        while first1 != last1 && first2 != last2 {
            if comp(a[first2], buffer[first1]) {
                a[result] = a[first2];
                first2 += 1;
            } else {
                a[result] = buffer[first1];
                first1 += 1;
            }
            result += 1;
        }
        while first1 != last1 {
            a[result] = buffer[first1];
            first1 += 1;
            result += 1;
        }
    } else {
        // buffer_end = move(middle, last, buffer)
        buffer[..len2].copy_from_slice(&a[middle..last]);
        // __move_merge_adaptive_backward(first, middle, buffer, buffer_end, last, comp)
        let first1 = first;
        let mut last1 = middle;
        let first2 = 0usize;
        let mut last2 = len2;
        let mut result = last;
        if first1 == last1 {
            // move_backward(first2, last2, result)
            while last2 != first2 {
                last2 -= 1;
                result -= 1;
                a[result] = buffer[last2];
            }
            return;
        } else if first2 == last2 {
            return;
        }
        last1 -= 1;
        last2 -= 1;
        loop {
            if comp(buffer[last2], a[last1]) {
                result -= 1;
                a[result] = a[last1];
                if first1 == last1 {
                    // move_backward(first2, ++last2, result)
                    let mut l2 = last2 + 1;
                    while l2 != first2 {
                        l2 -= 1;
                        result -= 1;
                        a[result] = buffer[l2];
                    }
                    return;
                }
                last1 -= 1;
            } else {
                result -= 1;
                a[result] = buffer[last2];
                if first2 == last2 {
                    return;
                }
                last2 -= 1;
            }
        }
    }
}

fn stable_sort_adaptive<F: FnMut(usize, usize) -> bool>(a: &mut [usize], first: usize, middle: usize, last: usize, buffer: &mut [usize], comp: &mut F) {
    idx_merge_sort_with_buffer(a, first, middle, buffer, comp);
    idx_merge_sort_with_buffer(a, middle, last, buffer, comp);
    idx_merge_adaptive(a, first, middle, last, middle - first, last - middle, buffer, comp);
}

fn introsort_loop<T, F>(v: &mut [T], first: usize, mut last: usize, mut depth_limit: usize, less: &mut F)
where
    F: FnMut(&T, &T) -> bool,
{
    while last - first > S_THRESHOLD {
        if depth_limit == 0 {
            partial_sort_impl(v, first, last, last, less);
            return;
        }
        depth_limit -= 1;
        let cut = unguarded_partition_pivot(v, first, last, less);
        introsort_loop(v, cut, last, depth_limit, less);
        last = cut;
    }
}

fn move_median_to_first<T, F>(v: &mut [T], result: usize, a: usize, b: usize, c: usize, less: &mut F)
where
    F: FnMut(&T, &T) -> bool,
{
    if less(&v[a], &v[b]) {
        if less(&v[b], &v[c]) {
            v.swap(result, b);
        } else if less(&v[a], &v[c]) {
            v.swap(result, c);
        } else {
            v.swap(result, a);
        }
    } else if less(&v[a], &v[c]) {
        v.swap(result, a);
    } else if less(&v[b], &v[c]) {
        v.swap(result, c);
    } else {
        v.swap(result, b);
    }
}

fn unguarded_partition<T, F>(v: &mut [T], mut first: usize, mut last: usize, pivot: usize, less: &mut F) -> usize
where
    F: FnMut(&T, &T) -> bool,
{
    loop {
        while less(&v[first], &v[pivot]) {
            first += 1;
        }
        last -= 1;
        while less(&v[pivot], &v[last]) {
            last -= 1;
        }
        if first >= last {
            return first;
        }
        v.swap(first, last);
        first += 1;
    }
}

fn unguarded_partition_pivot<T, F>(v: &mut [T], first: usize, last: usize, less: &mut F) -> usize
where
    F: FnMut(&T, &T) -> bool,
{
    let mid = first + (last - first) / 2;
    move_median_to_first(v, first, first + 1, mid, last - 1, less);
    unguarded_partition(v, first + 1, last, first, less)
}

fn final_insertion_sort<T, F>(v: &mut [T], first: usize, last: usize, less: &mut F)
where
    F: FnMut(&T, &T) -> bool,
{
    if last - first > S_THRESHOLD {
        insertion_sort(v, first, first + S_THRESHOLD, less);
        for i in first + S_THRESHOLD..last {
            unguarded_linear_insert(v, i, less);
        }
    } else {
        insertion_sort(v, first, last, less);
    }
}

fn insertion_sort<T, F>(v: &mut [T], first: usize, last: usize, less: &mut F)
where
    F: FnMut(&T, &T) -> bool,
{
    if first == last {
        return;
    }
    for i in first + 1..last {
        if less(&v[i], &v[first]) {
            // move_backward(first, i, i + 1); *first = val;
            v[first..=i].rotate_right(1);
        } else {
            unguarded_linear_insert(v, i, less);
        }
    }
}

fn unguarded_linear_insert<T, F>(v: &mut [T], last: usize, less: &mut F)
where
    F: FnMut(&T, &T) -> bool,
{
    // Find the final position first (comparisons are made against the value being inserted,
    // exactly as in libstdc++), then rotate it into place.
    let mut pos = last;
    while pos > 0 && less(&v[last], &v[pos - 1]) {
        pos -= 1;
    }
    if pos != last {
        v[pos..=last].rotate_right(1);
    }
}

// ---------------------------------------------------------------------------------------
// Heap algorithms (libstdc++ stl_heap.h)
// ---------------------------------------------------------------------------------------

/// Hole-based heap adjustment mirroring `std::__adjust_heap`.
/// `value` is the element that is conceptually removed from `hole_index`.
unsafe fn adjust_heap<T, F>(base: *mut T, mut hole_index: usize, len: usize, value: T, less: &mut F)
where
    F: FnMut(&T, &T) -> bool,
{
    let top_index = hole_index;
    let mut second_child = hole_index;
    while len > 0 && second_child < (len - 1) / 2 {
        second_child = 2 * (second_child + 1);
        if less(&*base.add(second_child), &*base.add(second_child - 1)) {
            second_child -= 1;
        }
        ptr::copy_nonoverlapping(base.add(second_child), base.add(hole_index), 1);
        hole_index = second_child;
    }
    if (len & 1) == 0 && len >= 2 && second_child == (len - 2) / 2 {
        second_child = 2 * (second_child + 1);
        ptr::copy_nonoverlapping(base.add(second_child - 1), base.add(hole_index), 1);
        hole_index = second_child - 1;
    }
    push_heap_hole(base, hole_index, top_index, value, less);
}

/// `std::__push_heap`: moves `value` up from `hole_index`.
unsafe fn push_heap_hole<T, F>(base: *mut T, mut hole_index: usize, top_index: usize, value: T, less: &mut F)
where
    F: FnMut(&T, &T) -> bool,
{
    while hole_index > top_index {
        let parent = (hole_index - 1) / 2;
        if !less(&*base.add(parent), &value) {
            break;
        }
        ptr::copy_nonoverlapping(base.add(parent), base.add(hole_index), 1);
        hole_index = parent;
    }
    ptr::write(base.add(hole_index), value);
}

/// `std::make_heap` (max-heap with respect to `less`).
pub fn make_heap_by<T, F>(v: &mut [T], mut less: F)
where
    F: FnMut(&T, &T) -> bool,
{
    make_heap_impl(v, 0, v.len(), &mut less);
}

fn make_heap_impl<T, F>(v: &mut [T], first: usize, last: usize, less: &mut F)
where
    F: FnMut(&T, &T) -> bool,
{
    let len = last - first;
    if len < 2 {
        return;
    }
    let mut parent = (len - 2) / 2;
    loop {
        unsafe {
            let base = v.as_mut_ptr().add(first);
            let value = ptr::read(base.add(parent));
            adjust_heap(base, parent, len, value, less);
        }
        if parent == 0 {
            return;
        }
        parent -= 1;
    }
}

/// `std::push_heap`: the new element is the last one of `v`.
pub fn push_heap_by<T, F>(v: &mut [T], mut less: F)
where
    F: FnMut(&T, &T) -> bool,
{
    let len = v.len();
    if len < 2 {
        return;
    }
    unsafe {
        let base = v.as_mut_ptr();
        let value = ptr::read(base.add(len - 1));
        push_heap_hole(base, len - 1, 0, value, &mut less);
    }
}

/// `std::pop_heap`: moves the top element to the back of `v`.
pub fn pop_heap_by<T, F>(v: &mut [T], mut less: F)
where
    F: FnMut(&T, &T) -> bool,
{
    let len = v.len();
    if len < 2 {
        return;
    }
    pop_heap_impl(v, 0, len - 1, len - 1, &mut less);
}

fn pop_heap_impl<T, F>(v: &mut [T], first: usize, last: usize, result: usize, less: &mut F)
where
    F: FnMut(&T, &T) -> bool,
{
    unsafe {
        let base = v.as_mut_ptr();
        let value = ptr::read(base.add(result));
        ptr::copy_nonoverlapping(base.add(first), base.add(result), 1);
        adjust_heap(base.add(first), 0, last - first, value, less);
    }
}

fn sort_heap_impl<T, F>(v: &mut [T], first: usize, mut last: usize, less: &mut F)
where
    F: FnMut(&T, &T) -> bool,
{
    while last - first > 1 {
        last -= 1;
        pop_heap_impl(v, first, last, last, less);
    }
}

/// `std::sort_heap`.
pub fn sort_heap_by<T, F>(v: &mut [T], mut less: F)
where
    F: FnMut(&T, &T) -> bool,
{
    let n = v.len();
    sort_heap_impl(v, 0, n, &mut less);
}

fn heap_select<T, F>(v: &mut [T], first: usize, middle: usize, last: usize, less: &mut F)
where
    F: FnMut(&T, &T) -> bool,
{
    make_heap_impl(v, first, middle, less);
    for i in middle..last {
        if less(&v[i], &v[first]) {
            pop_heap_impl(v, first, middle, i, less);
        }
    }
}

fn partial_sort_impl<T, F>(v: &mut [T], first: usize, middle: usize, last: usize, less: &mut F)
where
    F: FnMut(&T, &T) -> bool,
{
    heap_select(v, first, middle, last, less);
    sort_heap_impl(v, first, middle, less);
}

/// `std::partial_sort(first, first + middle, last)`.
pub fn partial_sort_by<T, F>(v: &mut [T], middle: usize, mut less: F)
where
    F: FnMut(&T, &T) -> bool,
{
    let n = v.len();
    partial_sort_impl(v, 0, middle, n, &mut less);
}

// ---------------------------------------------------------------------------------------
// nth_element (introselect)
// ---------------------------------------------------------------------------------------

/// `std::nth_element`.
pub fn nth_element_by<T, F>(v: &mut [T], nth: usize, mut less: F)
where
    F: FnMut(&T, &T) -> bool,
{
    let n = v.len();
    if n == 0 || nth == n {
        return;
    }
    let mut first = 0;
    let mut last = n;
    let mut depth_limit = lg(n) * 2;
    while last - first > 3 {
        if depth_limit == 0 {
            heap_select(v, first, nth + 1, last, &mut less);
            v.swap(first, nth);
            return;
        }
        depth_limit -= 1;
        let cut = unguarded_partition_pivot(v, first, last, &mut less);
        if cut <= nth {
            first = cut;
        } else {
            last = cut;
        }
    }
    insertion_sort(v, first, last, &mut less);
}

// ---------------------------------------------------------------------------------------
// priority_queue
// ---------------------------------------------------------------------------------------

/// `std::priority_queue<T, std::vector<T>, Compare>`: a max-heap with respect to `less`
/// whose tie-breaking matches libstdc++.
pub struct PriorityQueue<T, F: FnMut(&T, &T) -> bool> {
    data: Vec<T>,
    less: F,
}

impl<T, F: FnMut(&T, &T) -> bool> PriorityQueue<T, F> {
    pub fn new(less: F) -> Self {
        PriorityQueue { data: Vec::new(), less }
    }

    /// Constructs from a vector, like `priority_queue(comp, std::move(vec))` (calls make_heap).
    pub fn from_vec(mut data: Vec<T>, mut less: F) -> Self {
        let n = data.len();
        make_heap_impl(&mut data, 0, n, &mut less);
        PriorityQueue { data, less }
    }

    pub fn push(&mut self, value: T) {
        self.data.push(value);
        push_heap_by(&mut self.data, &mut self.less);
    }

    pub fn top(&self) -> Option<&T> {
        self.data.first()
    }

    pub fn pop(&mut self) -> Option<T> {
        if self.data.is_empty() {
            return None;
        }
        pop_heap_by(&mut self.data, &mut self.less);
        self.data.pop()
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn clear(&mut self) {
        self.data.clear();
    }
}

// ---------------------------------------------------------------------------------------
// min/max element with C++ semantics (first occurrence wins for both)
// ---------------------------------------------------------------------------------------

/// `std::min_element`: index of the first smallest element.
pub fn min_element_by<T, F: FnMut(&T, &T) -> bool>(v: &[T], mut less: F) -> Option<usize> {
    if v.is_empty() {
        return None;
    }
    let mut best = 0;
    for i in 1..v.len() {
        if less(&v[i], &v[best]) {
            best = i;
        }
    }
    Some(best)
}

/// `std::max_element`: index of the *first* largest element (Rust's `max_by` returns the last).
pub fn max_element_by<T, F: FnMut(&T, &T) -> bool>(v: &[T], mut less: F) -> Option<usize> {
    if v.is_empty() {
        return None;
    }
    let mut best = 0;
    for i in 1..v.len() {
        if less(&v[best], &v[i]) {
            best = i;
        }
    }
    Some(best)
}

/// `std::min_element` over an iterator, by key; returns the first element with the minimal key.
pub fn min_by_key_first<I, T, K, F>(iter: I, mut key: F) -> Option<T>
where
    I: IntoIterator<Item = T>,
    K: PartialOrd,
    F: FnMut(&T) -> K,
{
    let mut best: Option<(T, K)> = None;
    for item in iter {
        let k = key(&item);
        match &best {
            None => best = Some((item, k)),
            Some((_, bk)) => {
                if k < *bk {
                    best = Some((item, k));
                }
            }
        }
    }
    best.map(|(t, _)| t)
}

/// `std::max_element` over an iterator, by key; returns the first element with the maximal key.
pub fn max_by_key_first<I, T, K, F>(iter: I, mut key: F) -> Option<T>
where
    I: IntoIterator<Item = T>,
    K: PartialOrd,
    F: FnMut(&T) -> K,
{
    let mut best: Option<(T, K)> = None;
    for item in iter {
        let k = key(&item);
        match &best {
            None => best = Some((item, k)),
            Some((_, bk)) => {
                if *bk < k {
                    best = Some((item, k));
                }
            }
        }
    }
    best.map(|(t, _)| t)
}

/// `std::unique` followed by erase: removes consecutive duplicates using `eq`.
pub fn unique_by<T, F: FnMut(&T, &T) -> bool>(v: &mut Vec<T>, mut eq: F) {
    v.dedup_by(|b, a| eq(a, b));
}

/// C/C++ `llrint` with the default rounding mode (round half to even).
#[inline]
pub fn llrint(x: f64) -> i64 {
    x.round_ties_even() as i64
}

/// C/C++ `std::round` (half away from zero).
#[inline]
pub fn round(x: f64) -> f64 {
    x.round()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_sort_matches_reference_on_inconsistent_comparator() {
        // Compare against a straightforward stable sort for a strict weak ordering.
        let mut v: Vec<(i32, usize)> = (0..200).map(|i| (((i * 7919) % 31) as i32, i)).collect();
        let mut w = v.clone();
        super::stable_sort_by(&mut v, |a, b| a.0 < b.0);
        w.sort_by_key(|a| a.0);
        assert_eq!(v, w);
    }

    #[test]
    fn sorts_correctly() {
        let mut seed: u64 = 12345;
        for n in 0..300 {
            let mut v: Vec<(i32, usize)> = (0..n)
                .map(|i| {
                    seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                    (((seed >> 33) % 10) as i32, i)
                })
                .collect();
            sort_by(&mut v, |a, b| a.0 < b.0);
            for w in v.windows(2) {
                assert!(w[0].0 <= w[1].0);
            }
        }
    }

    #[test]
    fn priority_queue_order() {
        let mut pq = PriorityQueue::new(|a: &i32, b: &i32| a < b);
        for x in [5, 1, 9, 3, 7, 9, 2] {
            pq.push(x);
        }
        let mut out = vec![];
        while let Some(x) = pq.pop() {
            out.push(x);
        }
        assert_eq!(out, vec![9, 9, 7, 5, 3, 2, 1]);
    }

    #[test]
    fn nth() {
        let mut v = vec![9, 1, 8, 2, 7, 3, 6, 4, 5, 0, 11, 15, 13, 12, 14, 10, 19, 18, 17, 16];
        nth_element_by(&mut v, 7, |a, b| a < b);
        assert_eq!(v[7], 7);
    }
}

/// `std::lerp(a, b, t)` exactly as implemented by libstdc++ (C++20), or Microsoft's STL.
#[inline]
pub fn lerp(a: f64, b: f64, t: f64) -> f64 {
    if flavor::msvc() {
        return msvc::lerp(a, b, t);
    }
    if (a <= 0.0 && b >= 0.0) || (a >= 0.0 && b <= 0.0) {
        return t * b + (1.0 - t) * a;
    }
    if t == 1.0 {
        return b; // exact
    }
    // Exact at t=0, monotonic except near t=1, bounded, determinate, and consistent:
    let x = a + t * (b - a);
    if (t > 1.0) == (b > a) {
        if b < x {
            x
        } else {
            b
        }
    } else if b > x {
        x
    } else {
        b
    }
}

/// `std::lerp` for `float`.
#[inline]
pub fn lerpf(a: f32, b: f32, t: f32) -> f32 {
    if flavor::msvc() {
        return msvc::lerpf(a, b, t);
    }
    if (a <= 0.0 && b >= 0.0) || (a >= 0.0 && b <= 0.0) {
        return t * b + (1.0 - t) * a;
    }
    if t == 1.0 {
        return b;
    }
    let x = a + t * (b - a);
    if (t > 1.0) == (b > a) {
        if b < x {
            x
        } else {
            b
        }
    } else if b > x {
        x
    } else {
        b
    }
}

/// C `llround` (round half away from zero).
#[inline]
pub fn llround(x: f64) -> i64 {
    x.round() as i64
}

/// C `llrint` for floats (round half to even, computed in single precision).
#[inline]
pub fn llrintf(x: f32) -> i64 {
    x.round_ties_even() as i64
}

/// C `atof`/`strtod` prefix parsing: parses the longest valid floating point prefix (after
/// leading whitespace); returns 0.0 if there is none.
pub fn atof(s: &str) -> f64 {
    strtod_prefix(s).0
}

/// Returns (value, number of bytes consumed) like `strtod`.
pub fn strtod_prefix(s: &str) -> (f64, usize) {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() && (b[i] as char).is_ascii_whitespace() {
        i += 1;
    }
    let start = i;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    // inf / nan
    let rest = &s[i..];
    let lower: String = rest.chars().take(8).collect::<String>().to_ascii_lowercase();
    if lower.starts_with("infinity") {
        let neg = b.get(start) == Some(&b'-');
        return (if neg { f64::NEG_INFINITY } else { f64::INFINITY }, i + 8);
    }
    if lower.starts_with("inf") {
        let neg = b.get(start) == Some(&b'-');
        return (if neg { f64::NEG_INFINITY } else { f64::INFINITY }, i + 3);
    }
    if lower.starts_with("nan") {
        return (f64::NAN, i + 3);
    }
    let digits_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let mut n_digits = i - digits_start;
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let frac_start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        n_digits += i - frac_start;
    }
    if n_digits == 0 {
        return (0.0, 0);
    }
    // exponent
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let exp_digits_start = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > exp_digits_start {
            i = j;
        }
    }
    let text = &s[start..i];
    let text = text.strip_suffix('.').map(|t| format!("{}.0", t)).unwrap_or_else(|| text.to_string());
    (text.parse::<f64>().unwrap_or(0.0), i)
}

/// C `atoi`/`strtol` (base 10) prefix parsing. Out-of-range values saturate like strtol (then
/// truncate to int like atoi on glibc, which calls strtol and casts).
pub fn atoi(s: &str) -> i32 {
    atol(s) as i32
}

/// C `strtol(s, nullptr, 10)` prefix parsing with saturation.
pub fn atol(s: &str) -> i64 {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() && (b[i] as char).is_ascii_whitespace() {
        i += 1;
    }
    let mut neg = false;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        neg = b[i] == b'-';
        i += 1;
    }
    let mut v: i128 = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        v = v * 10 + (b[i] - b'0') as i128;
        if v > i64::MAX as i128 + 1 {
            v = i64::MAX as i128 + 1;
        }
        i += 1;
    }
    if neg {
        v = -v;
    }
    v.clamp(i64::MIN as i128, i64::MAX as i128) as i64
}

/// `std::stoul`: like strtoul, but a string without digits is an error (C++ throws).
pub fn stoul(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() && (b[i] as char).is_ascii_whitespace() {
        i += 1;
    }
    let mut neg = false;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        neg = b[i] == b'-';
        i += 1;
    }
    let ds = i;
    let mut v: u64 = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        v = v.wrapping_mul(10).wrapping_add((b[i] - b'0') as u64);
        i += 1;
    }
    if i == ds {
        return None;
    }
    Some(if neg { v.wrapping_neg() } else { v })
}

/// `std::to_string(double)`: printf("%f").
pub fn to_string_f64(v: f64) -> String {
    format!("{:.6}", v)
}

#[cfg(test)]
mod parse_tests {
    use super::*;
    #[test]
    fn parse() {
        assert_eq!(atof("0.4"), 0.4);
        assert_eq!(atof(" 12abc"), 12.0);
        assert_eq!(atof("True"), 0.0);
        assert_eq!(atof("-3.5e2x"), -350.0);
        assert_eq!(atof("5."), 5.0);
        assert_eq!(atoi("42.7"), 42);
        assert_eq!(atoi("-1"), -1);
        assert_eq!(stoul("3"), Some(3));
        assert_eq!(stoul("x"), None);
        assert_eq!(lerp(0.0, 10.0, 0.5), 5.0);
    }
}

/// glibc's `rand()`/`srand()` (the TYPE_3 additive feedback generator behind `random()`), with
/// the process-global state that the C library uses.
pub mod crand {
    use std::sync::Mutex;

    struct State {
        r: [i32; 34],
        // ring buffer position emulation: keep the last 34 values
        hist: Vec<i32>,
    }

    fn seeded(seed: u32) -> State {
        let mut r = vec![0i32; 344];
        let seed = if seed == 0 { 1 } else { seed };
        r[0] = seed as i32;
        for i in 1..31 {
            // r[i] = (16807 * r[i-1]) % 2147483647 computed without overflow (Schrage's method)
            let hi = r[i - 1] / 127773;
            let lo = r[i - 1] % 127773;
            let mut word = 16807i32.wrapping_mul(lo).wrapping_sub(2836i32.wrapping_mul(hi));
            if word < 0 {
                word = word.wrapping_add(2147483647);
            }
            r[i] = word;
        }
        for i in 31..34 {
            r[i] = r[i - 31];
        }
        for i in 34..344 {
            r[i] = r[i - 31].wrapping_add(r[i - 3]);
        }
        let mut st = State { r: [0; 34], hist: r[344 - 34..].to_vec() };
        st.r.copy_from_slice(&st.hist);
        st
    }

    impl State {
        fn next(&mut self) -> i32 {
            let n = self.hist.len();
            let v = self.hist[n - 31].wrapping_add(self.hist[n - 3]);
            self.hist.push(v);
            if self.hist.len() > 4096 {
                let keep = self.hist.split_off(self.hist.len() - 34);
                self.hist = keep;
            }
            ((v as u32) >> 1) as i32
        }
    }

    static STATE: Mutex<Option<State>> = Mutex::new(None);
    /// The Universal CRT's `_rand_state` (per thread in the CRT; Curust slices on one thread).
    static MSVC_STATE: Mutex<u32> = Mutex::new(1);

    /// `srand(seed)`.
    pub fn srand(seed: u32) {
        if crate::flavor::msvc() {
            *MSVC_STATE.lock().unwrap() = seed;
            return;
        }
        *STATE.lock().unwrap() = Some(seeded(seed));
    }

    /// `rand()`: returns a value in [0, rand_max()].
    pub fn rand() -> i32 {
        if crate::flavor::msvc() {
            // UCRT: state = state * 214013 + 2531011; return (state >> 16) & RAND_MAX
            let mut st = MSVC_STATE.lock().unwrap();
            *st = st.wrapping_mul(214013).wrapping_add(2531011);
            return ((*st >> 16) & 0x7fff) as i32;
        }
        let mut guard = STATE.lock().unwrap();
        let st = guard.get_or_insert_with(|| seeded(1));
        st.next()
    }

    /// glibc's `RAND_MAX`.
    pub const RAND_MAX: i32 = 2147483647;

    /// `RAND_MAX` of the emulated C library (glibc 2147483647, UCRT 32767).
    pub fn rand_max() -> i32 {
        if crate::flavor::msvc() {
            0x7fff
        } else {
            RAND_MAX
        }
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn matches_glibc_default_sequence() {
            super::srand(1);
            // First values of glibc rand() with the default seed.
            assert_eq!(super::rand(), 1804289383);
            assert_eq!(super::rand(), 846930886);
            assert_eq!(super::rand(), 1681692777);
        }
    }
}
