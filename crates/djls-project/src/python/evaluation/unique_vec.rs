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

    pub(crate) fn iter(&self) -> std::slice::Iter<'_, T> {
        self.0.iter()
    }

    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }

    pub(crate) fn retain(&mut self, predicate: impl FnMut(&T) -> bool) {
        self.0.retain(predicate);
    }

    pub(crate) fn sort_by_key<K: Ord>(&mut self, key: impl FnMut(&T) -> K) {
        self.0.sort_by_key(key);
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
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a, T> IntoIterator for &'a UniqueVec<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}
