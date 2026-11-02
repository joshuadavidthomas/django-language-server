use std::cmp::Ordering;
use std::slice::Iter;
use std::vec::IntoIter;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UniqueVec<T>(Vec<T>);

impl<T> UniqueVec<T> {
    pub(crate) const fn new() -> Self {
        Self(Vec::new())
    }

    #[cfg(test)]
    pub(crate) fn as_slice(&self) -> &[T] {
        &self.0
    }

    pub(crate) fn iter(&self) -> Iter<'_, T> {
        self.0.iter()
    }

    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }

    pub(crate) fn retain(&mut self, predicate: impl FnMut(&T) -> bool) {
        self.0.retain(predicate);
    }

    pub(crate) fn sort_by(&mut self, compare: impl FnMut(&T, &T) -> Ordering) {
        self.0.sort_by(compare);
    }
}

impl<T: Eq> UniqueVec<T> {
    pub(crate) fn insert(&mut self, element: T) -> bool {
        if self.0.contains(&element) {
            false
        } else {
            self.0.push(element);
            true
        }
    }
}

impl<T: Eq + Clone> UniqueVec<T> {
    /// Adds the elements of `other` this vector lacks, in order. Forks start
    /// as copies, so a shared prefix is skipped without searching or cloning.
    /// `other` is already unique, so its remaining elements can only repeat
    /// this vector's own elements past that prefix, never each other.
    pub(crate) fn absorb(&mut self, other: &Self) {
        let shared = self
            .0
            .iter()
            .zip(&other.0)
            .take_while(|(left, right)| left == right)
            .count();
        let existing = self.0.len();
        for element in &other.0[shared..] {
            if !self.0[shared..existing].contains(element) {
                self.0.push(element.clone());
            }
        }
    }
}

impl<T> Default for UniqueVec<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Eq> Extend<T> for UniqueVec<T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        for element in iter {
            self.insert(element);
        }
    }
}

impl<T: Eq> FromIterator<T> for UniqueVec<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let mut values = Self::new();
        values.extend(iter);
        values
    }
}

impl<T: Eq> From<Vec<T>> for UniqueVec<T> {
    fn from(value: Vec<T>) -> Self {
        value.into_iter().collect()
    }
}

impl<T> IntoIterator for UniqueVec<T> {
    type Item = T;
    type IntoIter = IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a, T> IntoIterator for &'a UniqueVec<T> {
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::UniqueVec;

    #[test]
    fn absorb_appends_only_missing_elements_in_order() {
        let mut base = UniqueVec::from(vec!["a", "b", "c", "d"]);
        let fork = UniqueVec::from(vec!["a", "b", "x", "d", "y"]);

        base.absorb(&fork);

        assert_eq!(base.as_slice(), ["a", "b", "c", "d", "x", "y"]);
    }

    #[test]
    fn absorb_into_empty_copies_every_element() {
        let mut base = UniqueVec::new();

        base.absorb(&UniqueVec::from(vec![3, 1, 2]));

        assert_eq!(base.as_slice(), [3, 1, 2]);
    }
}
