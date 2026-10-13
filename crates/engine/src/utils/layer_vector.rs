//! `utils/LayerVector.h`: a vector indexed by layer number, which may start at a negative raft layer.

use crate::raft::Raft;
use crate::settings::types::LayerIndex;

#[derive(Clone, Debug)]
pub struct LayerVector<T> {
    delta: usize,
    vector: Vec<T>,
}

impl<T> Default for LayerVector<T> {
    fn default() -> Self {
        LayerVector { delta: 0, vector: Vec::new() }
    }
}

impl<T> LayerVector<T> {
    pub fn init(&mut self, contains_raft_layers: bool, print_layers: usize) {
        self.delta = if contains_raft_layers { Raft::get_total_extra_layers() } else { 0 };
        self.vector.clear();
        if print_layers != 0 {
            self.vector.reserve(print_layers + self.delta);
        }
    }

    pub fn push(&mut self, item: T) {
        self.vector.push(item);
    }

    /// `operator[](pos)`
    pub fn get_ref(&self, pos: LayerIndex) -> &T {
        &self.vector[(pos as i64 + self.delta as i64) as usize]
    }

    pub fn get_mut(&mut self, pos: LayerIndex) -> &mut T {
        &mut self.vector[(pos as i64 + self.delta as i64) as usize]
    }

    /// `iterator_at(pos)`: the storage index of the layer, if it exists.
    pub fn index_at(&self, pos: LayerIndex) -> Option<usize> {
        let index = pos as i64 + self.delta as i64;
        if index >= 0 && (index as usize) < self.vector.len() {
            Some(index as usize)
        } else {
            None
        }
    }

    /// `getLayer(iterator)`: the layer number of a storage index.
    pub fn layer_of(&self, index: usize) -> LayerIndex {
        (index as i64 - self.delta as i64) as LayerIndex
    }

    pub fn at_index(&self, index: usize) -> &T {
        &self.vector[index]
    }

    pub fn at_index_mut(&mut self, index: usize) -> &mut T {
        &mut self.vector[index]
    }

    pub fn front(&self) -> &T {
        &self.vector[0]
    }

    pub fn iter(&self) -> std::slice::Iter<'_, T> {
        self.vector.iter()
    }

    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, T> {
        self.vector.iter_mut()
    }

    pub fn len(&self) -> usize {
        self.vector.len()
    }

    pub fn is_empty(&self) -> bool {
        self.vector.is_empty()
    }
}

impl<T: Clone + Default> LayerVector<T> {
    /// `get(pos)`: a copy of the element, or a default value when out of range.
    pub fn get(&self, pos: LayerIndex) -> T {
        match self.index_at(pos) {
            Some(i) => self.vector[i].clone(),
            None => T::default(),
        }
    }
}
