//! Microsoft STL (`<algorithm>`, VS 2022) versions of the algorithms whose results depend on the
//! implementation, translated from microsoft/STL (Apache-2.0 WITH LLVM-exception).

const ISORT_MAX: usize = 32;

/// `_Insertion_sort_unchecked(first, last)`.
fn insertion_sort<T, F: FnMut(&T, &T) -> bool>(v: &mut [T], first: usize, last: usize, less: &mut F) {
    if first == last {
        return;
    }
    let mut mid = first + 1;
    while mid != last {
        if less(&v[mid], &v[first]) {
            // found new earliest element, move to front
            v[first..=mid].rotate_right(1);
        } else {
            // look for insertion point after first
            let mut hole = mid;
            while less(&v[mid], &v[hole - 1]) {
                hole -= 1;
            }
            if hole != mid {
                v[hole..=mid].rotate_right(1);
            }
        }
        mid += 1;
    }
}

/// `_Med3_unchecked`: sort median of three elements to middle.
fn med3<T, F: FnMut(&T, &T) -> bool>(v: &mut [T], first: usize, mid: usize, last: usize, less: &mut F) {
    if less(&v[mid], &v[first]) {
        v.swap(mid, first);
    }
    if less(&v[last], &v[mid]) {
        v.swap(last, mid);
        if less(&v[mid], &v[first]) {
            v.swap(mid, first);
        }
    }
}

/// `_Guess_median_unchecked` (Tukey's ninther above 40 elements); `last` is inclusive.
fn guess_median<T, F: FnMut(&T, &T) -> bool>(v: &mut [T], first: usize, mid: usize, last: usize, less: &mut F) {
    let count = last - first;
    if 40 < count {
        let step = (count + 1) >> 3;
        let two_step = step << 1;
        med3(v, first, first + step, first + two_step, less);
        med3(v, mid - step, mid, mid + step, less);
        med3(v, last - two_step, last - step, last, less);
        med3(v, first + step, mid, last - step, less);
    } else {
        med3(v, first, mid, last, less);
    }
}

/// `_Partition_by_pivot_unchecked`: three-way partition around the pivot at `pfirst`; returns the
/// range of elements equivalent to the pivot.
fn partition_by_pivot<T, F: FnMut(&T, &T) -> bool>(v: &mut [T], first: usize, mut pfirst: usize, last: usize, less: &mut F) -> (usize, usize) {
    let mut plast = pfirst + 1;
    while first < pfirst && !less(&v[pfirst - 1], &v[pfirst]) && !less(&v[pfirst], &v[pfirst - 1]) {
        pfirst -= 1;
    }
    while plast < last && !less(&v[plast], &v[pfirst]) && !less(&v[pfirst], &v[plast]) {
        plast += 1;
    }
    let mut gfirst = plast;
    let mut glast = pfirst;
    loop {
        while gfirst < last {
            if less(&v[pfirst], &v[gfirst]) {
                gfirst += 1;
                continue;
            } else if less(&v[gfirst], &v[pfirst]) {
                break;
            } else if plast != gfirst {
                v.swap(plast, gfirst);
                plast += 1;
            } else {
                plast += 1;
            }
            gfirst += 1;
        }
        while first < glast {
            let glast_prev = glast - 1;
            if less(&v[glast_prev], &v[pfirst]) {
                glast -= 1;
                continue;
            } else if less(&v[pfirst], &v[glast_prev]) {
                break;
            } else {
                pfirst -= 1;
                if pfirst != glast_prev {
                    v.swap(pfirst, glast_prev);
                }
            }
            glast -= 1;
        }
        if glast == first && gfirst == last {
            return (pfirst, plast);
        }
        if glast == first {
            // no room at bottom, rotate pivot upward
            if plast != gfirst {
                v.swap(pfirst, plast);
            }
            plast += 1;
            v.swap(pfirst, gfirst);
            pfirst += 1;
            gfirst += 1;
        } else if gfirst == last {
            // no room at top, rotate pivot downward
            glast -= 1;
            pfirst -= 1;
            if glast != pfirst {
                v.swap(glast, pfirst);
            }
            plast -= 1;
            v.swap(pfirst, plast);
        } else {
            glast -= 1;
            v.swap(gfirst, glast);
            gfirst += 1;
        }
    }
}

/// `_Partition_by_median_guess_unchecked`.
fn partition_by_median_guess<T, F: FnMut(&T, &T) -> bool>(v: &mut [T], first: usize, last: usize, less: &mut F) -> (usize, usize) {
    let mid = first + ((last - first) >> 1);
    guess_median(v, first, mid, last - 1, less);
    partition_by_pivot(v, first, mid, last, less)
}

/// `_Sort_unchecked(first, last, ideal)`.
fn sort_unchecked<T, F: FnMut(&T, &T) -> bool>(v: &mut [T], mut first: usize, mut last: usize, mut ideal: isize, less: &mut F) {
    loop {
        if last - first <= ISORT_MAX {
            insertion_sort(v, first, last, less);
            return;
        }
        if ideal <= 0 {
            // heap sort if too many divisions (MSVC's heap algorithms equal libstdc++'s)
            super::make_heap_by(&mut v[first..last], &mut *less);
            super::sort_heap_by(&mut v[first..last], &mut *less);
            return;
        }
        let (mid_first, mid_last) = partition_by_median_guess(v, first, last, less);
        ideal = (ideal >> 1) + (ideal >> 2); // allow 1.5 log2(N) divisions
        if mid_first - first < last - mid_last {
            // loop on second half
            sort_unchecked(v, first, mid_first, ideal, less);
            first = mid_last;
        } else {
            // loop on first half
            sort_unchecked(v, mid_last, last, ideal, less);
            last = mid_first;
        }
    }
}

/// `std::sort(first, last, pred)`.
pub fn sort_by<T, F: FnMut(&T, &T) -> bool>(v: &mut [T], less: &mut F) {
    let n = v.len();
    sort_unchecked(v, 0, n, n as isize, less);
}

// ---------------------------------------------------------------------------------------------
// std::stable_sort (_Stable_sort_unchecked with a temporary buffer of ceil(n/2) elements)
// ---------------------------------------------------------------------------------------------

/// Insertion sort of the index range `a[first..last]` (`_Insertion_sort_unchecked`).
fn idx_insertion_sort<F: FnMut(usize, usize) -> bool>(a: &mut [usize], first: usize, last: usize, cmp: &mut F) {
    if first == last {
        return;
    }
    let mut mid = first + 1;
    while mid != last {
        let val = a[mid];
        if cmp(val, a[first]) {
            a.copy_within(first..mid, first + 1);
            a[first] = val;
        } else {
            let mut hole = mid;
            while cmp(val, a[hole - 1]) {
                a[hole] = a[hole - 1];
                hole -= 1;
            }
            a[hole] = val;
        }
        mid += 1;
    }
}

/// `_Merge_move_unchecked` / `_Uninitialized_merge_move`: merge `src[first..mid]` and
/// `src[mid..last]` (both non-empty) into `dst` from `out`; returns the new `out`.
fn idx_merge_move<F: FnMut(usize, usize) -> bool>(src: &[usize], mut first: usize, mid: usize, last: usize, dst: &mut [usize], mut out: usize, cmp: &mut F) -> usize {
    let mut next = mid;
    loop {
        if cmp(src[next], src[first]) {
            dst[out] = src[next];
            out += 1;
            next += 1;
            if next == last {
                for i in first..mid {
                    dst[out] = src[i];
                    out += 1;
                }
                return out;
            }
        } else {
            dst[out] = src[first];
            out += 1;
            first += 1;
            if first == mid {
                for i in next..last {
                    dst[out] = src[i];
                    out += 1;
                }
                return out;
            }
        }
    }
}

/// `_Chunked_merge_unchecked(src[first..first+count] -> dst[out..], chunk)`.
fn idx_chunked_merge<F: FnMut(usize, usize) -> bool>(src: &[usize], mut first: usize, last: usize, dst: &mut [usize], mut out: usize, chunk: usize, mut count: usize, cmp: &mut F) {
    while chunk < count {
        count -= chunk;
        let mid1 = first + chunk;
        let chunk2 = chunk.min(count);
        count -= chunk2;
        let mid2 = mid1 + chunk2;
        out = idx_merge_move(src, first, mid1, mid2, dst, out, cmp);
        first = mid2;
    }
    for i in first..last {
        dst[out] = src[i];
        out += 1;
    }
}

/// `_Buffered_merge_sort_unchecked(a[first..last], count)` using `temp`.
fn idx_buffered_merge_sort<F: FnMut(usize, usize) -> bool>(a: &mut [usize], first: usize, last: usize, count: usize, temp: &mut [usize], cmp: &mut F) {
    // _Insertion_sort_isort_max_chunks
    {
        let mut f = first;
        let mut c = count;
        while ISORT_MAX < c {
            idx_insertion_sort(a, f, f + ISORT_MAX, cmp);
            f += ISORT_MAX;
            c -= ISORT_MAX;
        }
        idx_insertion_sort(a, f, last, cmp);
    }
    if count <= ISORT_MAX {
        return;
    }
    // _Uninitialized_chunked_merge_unchecked2: a -> temp with chunks of ISORT_MAX
    {
        let src: Vec<usize> = a[first..last].to_vec();
        idx_chunked_merge(&src, 0, count, temp, 0, ISORT_MAX, count, cmp);
    }
    let mut chunk = ISORT_MAX;
    loop {
        chunk <<= 1;
        let tsrc: Vec<usize> = temp[..count].to_vec();
        idx_chunked_merge(&tsrc, 0, count, &mut a[first..last], 0, chunk, count, cmp);
        if (count - 1) / 2 < chunk {
            return;
        }
        chunk <<= 1;
        let src: Vec<usize> = a[first..last].to_vec();
        idx_chunked_merge(&src, 0, count, temp, 0, chunk, count, cmp);
    }
}

/// `_Inplace_merge_buffer_left`.
fn idx_merge_buffer_left<F: FnMut(usize, usize) -> bool>(a: &mut [usize], mut first: usize, mut mid: usize, last: usize, cmp: &mut F) {
    let tmp: Vec<usize> = a[first..mid].to_vec();
    let mut left_first = 0usize;
    let left_last = tmp.len() - 1; // avoid a compare with the highest element
    a[first] = a[mid]; // the lowest element is now in position
    first += 1;
    mid += 1;
    loop {
        if cmp(a[mid], tmp[left_first]) {
            a[first] = a[mid];
            first += 1;
            mid += 1;
            if mid == last {
                for &x in &tmp[left_first..] {
                    a[first] = x;
                    first += 1;
                }
                return;
            }
        } else {
            a[first] = tmp[left_first];
            first += 1;
            left_first += 1;
            if left_first == left_last {
                for i in mid..last {
                    a[first] = a[i];
                    first += 1;
                }
                a[first] = tmp[left_last];
                return;
            }
        }
    }
}

/// `_Inplace_merge_buffer_right`.
fn idx_merge_buffer_right<F: FnMut(usize, usize) -> bool>(a: &mut [usize], first: usize, mut mid: usize, mut last: usize, cmp: &mut F) {
    let tmp: Vec<usize> = a[mid..last].to_vec();
    last -= 1;
    mid -= 1;
    a[last] = a[mid]; // move the highest element into position
    let right_first = 0usize;
    let mut right_last = tmp.len() - 1;
    mid -= 1;
    loop {
        if cmp(tmp[right_last], a[mid]) {
            // merge from the left partition
            last -= 1;
            a[last] = a[mid];
            if first == mid {
                last -= 1;
                a[last] = tmp[right_last];
                // move_backward(right_first, right_last, last)
                let mut r = right_last;
                while r != right_first {
                    r -= 1;
                    last -= 1;
                    a[last] = tmp[r];
                }
                return;
            }
            mid -= 1;
        } else {
            // merge from the right partition
            last -= 1;
            a[last] = tmp[right_last];
            right_last -= 1;
            if right_first == right_last {
                last -= 1;
                a[last] = a[mid]; // restore half-open range [first, mid)
                // move_backward(first, mid, last)
                let mut m = mid;
                while m != first {
                    m -= 1;
                    last -= 1;
                    a[last] = a[m];
                }
                a[first] = tmp[right_first];
                return;
            }
        }
    }
}

/// `_Buffered_inplace_merge_unchecked` (the buffer always holds the smaller half here).
fn idx_buffered_inplace_merge<F: FnMut(usize, usize) -> bool>(a: &mut [usize], mut first: usize, mid: usize, mut last: usize, mut count1: usize, mut count2: usize, cap: usize, cmp: &mut F) {
    if mid == last {
        return;
    }
    loop {
        if first == mid {
            return;
        }
        if cmp(a[mid], a[first]) {
            break;
        }
        first += 1;
        count1 -= 1;
    }
    let highest = mid - 1;
    loop {
        last -= 1;
        count2 -= 1;
        if mid == last {
            // _Rotate_one_right(first, mid, last + 1)
            a[first..=mid].rotate_right(1);
            return;
        }
        if cmp(a[last], a[highest]) {
            break;
        }
    }
    last += 1;
    count2 += 1;
    if count1 == 1 {
        // _Rotate_one_left(first, mid, last)
        a[first..last].rotate_left(1);
        return;
    }
    if count1 <= count2 && count1 <= cap {
        idx_merge_buffer_left(a, first, mid, last, cmp);
    } else if count2 <= cap {
        idx_merge_buffer_right(a, first, mid, last, cmp);
    } else {
        unreachable!("stable_sort buffer smaller than half the range");
    }
}

/// `_Stable_sort_unchecked`.
fn idx_stable_sort<F: FnMut(usize, usize) -> bool>(a: &mut [usize], first: usize, last: usize, count: usize, temp: &mut [usize], cap: usize, cmp: &mut F) {
    if count <= ISORT_MAX {
        idx_insertion_sort(a, first, last, cmp);
        return;
    }
    let half = count >> 1;
    let half_ceil = count - half;
    let mid = first + half_ceil;
    if half_ceil <= cap {
        idx_buffered_merge_sort(a, first, mid, half_ceil, temp, cmp);
        idx_buffered_merge_sort(a, mid, last, half, temp, cmp);
    } else {
        idx_stable_sort(a, first, mid, half_ceil, temp, cap, cmp);
        idx_stable_sort(a, mid, last, half, temp, cap, cmp);
    }
    idx_buffered_inplace_merge(a, first, mid, last, half_ceil, half, cap, cmp);
}

/// `std::stable_sort(first, last, pred)`: the permutation that sorts `v`.
pub fn stable_sort_perm<T, F: FnMut(&T, &T) -> bool>(v: &[T], less: &mut F) -> Vec<usize> {
    let n = v.len();
    let mut idx: Vec<usize> = (0..n).collect();
    let mut cmp = |x: usize, y: usize| less(&v[x], &v[y]);
    if n <= ISORT_MAX {
        idx_insertion_sort(&mut idx, 0, n, &mut cmp);
        return idx;
    }
    let cap = n - n / 2;
    let mut temp = vec![0usize; cap];
    idx_stable_sort(&mut idx, 0, n, n, &mut temp, cap, &mut cmp);
    idx
}

// ---------------------------------------------------------------------------------------------
// std::list::sort (_List_val::_Sort: top-down merge sort, first half = size / 2)
// ---------------------------------------------------------------------------------------------

/// `std::list<T>::sort(pred)`: the permutation that sorts `v`.
pub fn list_sort_perm<T, F: FnMut(&T, &T) -> bool>(v: &[T], less: &mut F) -> Vec<usize> {
    fn sort_rec<F: FnMut(usize, usize) -> bool>(a: &mut [usize], cmp: &mut F) {
        let size = a.len();
        if size < 2 {
            return;
        }
        let half = size / 2;
        sort_rec(&mut a[..half], cmp);
        sort_rec(&mut a[half..], cmp);
        // _Merge_same: splices runs of the second range before the first range's elements; the
        // result is the stable merge that takes from the second range when pred(second, first).
        let left = a[..half].to_vec();
        let right = a[half..].to_vec();
        let (mut i, mut j, mut o) = (0usize, 0usize, 0usize);
        while i < left.len() && j < right.len() {
            if cmp(right[j], left[i]) {
                a[o] = right[j];
                j += 1;
            } else {
                a[o] = left[i];
                i += 1;
            }
            o += 1;
        }
        for &x in &left[i..] {
            a[o] = x;
            o += 1;
        }
        for &x in &right[j..] {
            a[o] = x;
            o += 1;
        }
    }
    let mut idx: Vec<usize> = (0..v.len()).collect();
    let mut cmp = |x: usize, y: usize| less(&v[x], &v[y]);
    sort_rec(&mut idx, &mut cmp);
    idx
}

// ---------------------------------------------------------------------------------------------
// <cmath> std::lerp (`_Common_lerp`: fma for the general case)
// ---------------------------------------------------------------------------------------------

/// `std::lerp(a, b, t)` for `double`.
pub fn lerp(a: f64, b: f64, t: f64) -> f64 {
    if t.is_finite() && a.is_finite() && b.is_finite() {
        if (a <= 0.0 && b >= 0.0) || (a >= 0.0 && b <= 0.0) {
            return t * b + (1.0 - t) * a;
        }
        if t == 1.0 {
            return b;
        }
        let candidate = t.mul_add(b - a, a);
        if (t > 1.0) == (b > a) {
            if b > candidate {
                return b;
            }
        } else if candidate > b {
            return b;
        }
        return candidate;
    }
    if a.is_nan() || b.is_nan() {
        return (a + b) + t;
    }
    if t.is_nan() {
        return t + t;
    }
    if t.is_finite() {
        if t < 0.0 {
            a - b
        } else if t <= 1.0 {
            t * b + (1.0 - t) * a
        } else {
            b - a
        }
    } else {
        t * (b - a)
    }
}

/// `std::lerp(a, b, t)` for `float`.
pub fn lerpf(a: f32, b: f32, t: f32) -> f32 {
    if t.is_finite() && a.is_finite() && b.is_finite() {
        if (a <= 0.0 && b >= 0.0) || (a >= 0.0 && b <= 0.0) {
            return t * b + (1.0 - t) * a;
        }
        if t == 1.0 {
            return b;
        }
        let candidate = t.mul_add(b - a, a);
        if (t > 1.0) == (b > a) {
            if b > candidate {
                return b;
            }
        } else if candidate > b {
            return b;
        }
        return candidate;
    }
    if a.is_nan() || b.is_nan() {
        return (a + b) + t;
    }
    if t.is_nan() {
        return t + t;
    }
    if t.is_finite() {
        if t < 0.0 {
            a - b
        } else if t <= 1.0 {
            t * b + (1.0 - t) * a
        } else {
            b - a
        }
    } else {
        t * (b - a)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sorts() {
        let mut seed: u64 = 99;
        for n in 0..500 {
            let mut v: Vec<(u32, usize)> = (0..n)
                .map(|i| {
                    seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                    (((seed >> 33) % 7) as u32, i)
                })
                .collect();
            super::sort_by(&mut v, &mut |a: &(u32, usize), b: &(u32, usize)| a.0 < b.0);
            assert!(v.windows(2).all(|w| w[0].0 <= w[1].0), "n={}", n);
            let mut ids: Vec<usize> = v.iter().map(|x| x.1).collect();
            ids.sort();
            assert_eq!(ids, (0..n).collect::<Vec<_>>());
        }
    }
}
