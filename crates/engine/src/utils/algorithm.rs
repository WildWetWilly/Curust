//! `utils/algorithm.h`.

/// Returns the indices of `input` in the order that sorts the values (stable).
pub fn order<T: PartialOrd>(input: &[T]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..input.len()).collect();
    cppstd::stable_sort_by(&mut order, |&i1, &i2| input[i1] < input[i2]);
    order
}
