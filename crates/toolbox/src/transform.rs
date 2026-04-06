//! Data transformation utilities

use std::collections::HashMap;

/// Transform a value with a function
pub fn transform<T, U, F>(value: T, f: F) -> U
where
    F: FnOnce(T) -> U,
{
    f(value)
}

/// Chain transformations
pub fn pipe<T>(value: T) -> Pipe<T> {
    Pipe(value)
}

/// Pipe for chaining transformations
pub struct Pipe<T>(T);

impl<T> Pipe<T> {
    /// Apply transformation
    pub fn then<U, F>(self, f: F) -> Pipe<U>
    where
        F: FnOnce(T) -> U,
    {
        Pipe(f(self.0))
    }

    /// Get result
    pub fn into_inner(self) -> T {
        self.0
    }
}

/// Map over collection with index
pub fn map_indexed<T, U, F>(items: Vec<T>, f: F) -> Vec<U>
where
    F: Fn(usize, T) -> U,
{
    items.into_iter().enumerate().map(|(i, v)| f(i, v)).collect()
}

/// Filter map operation
pub fn filter_map<T, U, F>(items: Vec<T>, f: F) -> Vec<U>
where
    F: Fn(T) -> Option<U>,
{
    items.into_iter().filter_map(f).collect()
}

/// Group by key
pub fn group_by<T, K, F>(items: Vec<T>, key_fn: F) -> HashMap<K, Vec<T>>
where
    K: std::hash::Hash + Eq,
    F: Fn(&T) -> K,
{
    let mut groups: HashMap<K, Vec<T>> = HashMap::new();
    
    for item in items {
        let key = key_fn(&item);
        groups.entry(key).or_default().push(item);
    }
    
    groups
}

/// Partition collection
pub fn partition<T, F>(items: Vec<T>, predicate: F) -> (Vec<T>, Vec<T>)
where
    F: Fn(&T) -> bool,
{
    let mut matching = Vec::new();
    let mut non_matching = Vec::new();
    
    for item in items {
        if predicate(&item) {
            matching.push(item);
        } else {
            non_matching.push(item);
        }
    }
    
    (matching, non_matching)
}

/// Chunk into groups of size n
pub fn chunks<T>(items: Vec<T>, size: usize) -> Vec<Vec<T>> {
    if size == 0 {
        return vec![items];
    }
    
    let mut result = Vec::new();
    let mut current = Vec::new();
    
    for item in items {
        if current.len() >= size {
            result.push(current);
            current = Vec::new();
        }
        current.push(item);
    }
    
    if !current.is_empty() {
        result.push(current);
    }
    
    result
}

/// Interleave two collections
pub fn interleave<T>(a: Vec<T>, b: Vec<T>) -> Vec<T> {
    let mut result = Vec::new();
    let mut a_iter = a.into_iter();
    let mut b_iter = b.into_iter();
    
    loop {
        match (a_iter.next(), b_iter.next()) {
            (Some(x), Some(y)) => {
                result.push(x);
                result.push(y);
            }
            (Some(x), None) => result.push(x),
            (None, Some(y)) => result.push(y),
            (None, None) => break,
        }
    }
    
    result
}

/// Deduplicate while preserving order
pub fn dedup<T>(items: Vec<T>) -> Vec<T>
where
    T: Eq + std::hash::Hash + Clone,
{
    use std::collections::HashSet;
    
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    
    for item in items {
        if seen.insert(item.clone()) {
            result.push(item);
        }
    }
    
    result
}

/// Flatten nested vector
pub fn flatten<T>(nested: Vec<Vec<T>>) -> Vec<T> {
    nested.into_iter().flatten().collect()
}

/// Transpose a matrix
pub fn transpose<T: Clone>(matrix: Vec<Vec<T>>) -> Vec<Vec<T>> {
    if matrix.is_empty() {
        return Vec::new();
    }
    
    let rows = matrix.len();
    let cols = matrix[0].len();
    
    let mut result = Vec::with_capacity(cols);
    
    for c in 0..cols {
        let mut row = Vec::with_capacity(rows);
        for r in 0..rows {
            if c < matrix[r].len() {
                row.push(matrix[r][c].clone());
            }
        }
        result.push(row);
    }
    
    result
}

/// Zip multiple collections
pub fn zip_many<T: Clone>(collections: Vec<Vec<T>>) -> Vec<Vec<T>> {
    if collections.is_empty() {
        return Vec::new();
    }
    
    let min_len = collections.iter().map(|c| c.len()).min().unwrap_or(0);
    
    (0..min_len)
        .map(|i| collections.iter().filter_map(|c| c.get(i).cloned()).collect())
        .collect()
}

/// Reverse key-value pairs
pub fn reverse_map<K, V>(map: HashMap<K, V>) -> HashMap<V, K>
where
    K: std::hash::Hash + Eq,
    V: std::hash::Hash + Eq,
{
    map.into_iter().map(|(k, v)| (v, k)).collect()
}

/// Merge maps with conflict resolution
pub fn merge_with<K, V, F>(a: HashMap<K, V>, b: HashMap<K, V>, resolve: F) -> HashMap<K, V>
where
    K: std::hash::Hash + Eq,
    F: Fn(V, V) -> V,
{
    let mut result = a;
    
    for (k, v) in b {
        match result.remove(&k) {
            Some(existing) => {
                result.insert(k, resolve(existing, v));
            }
            None => {
                result.insert(k, v);
            }
        }
    }
    
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipe() {
        let result = pipe(5)
            .then(|x| x * 2)
            .then(|x| x + 1)
            .into_inner();
        
        assert_eq!(result, 11);
    }

    #[test]
    fn test_group_by() {
        let items = vec![1, 2, 3, 4, 5, 6];
        let grouped = group_by(items, |x| x % 2 == 0);
        
        assert_eq!(grouped[&true], vec![2, 4, 6]);
        assert_eq!(grouped[&false], vec![1, 3, 5]);
    }

    #[test]
    fn test_partition() {
        let items = vec![1, 2, 3, 4, 5];
        let (even, odd) = partition(items, |x| x % 2 == 0);
        
        assert_eq!(even, vec![2, 4]);
        assert_eq!(odd, vec![1, 3, 5]);
    }

    #[test]
    fn test_chunks() {
        let items = vec![1, 2, 3, 4, 5];
        let chunked = chunks(items, 2);
        
        assert_eq!(chunked, vec![vec![1, 2], vec![3, 4], vec![5]]);
    }

    #[test]
    fn test_dedup() {
        let items = vec![1, 2, 2, 3, 3, 3];
        let deduped = dedup(items);
        
        assert_eq!(deduped, vec![1, 2, 3]);
    }

    #[test]
    fn test_flatten() {
        let nested = vec![vec![1, 2], vec![3, 4]];
        let flat = flatten(nested);
        
        assert_eq!(flat, vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_transpose() {
        let matrix = vec![
            vec![1, 2, 3],
            vec![4, 5, 6],
        ];
        let transposed = transpose(matrix);
        
        assert_eq!(transposed, vec![
            vec![1, 4],
            vec![2, 5],
            vec![3, 6],
        ]);
    }
}
