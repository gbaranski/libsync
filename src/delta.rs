use crate::frame::SequenceNumber;
use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;
use std::hash::Hash;

#[derive(Debug, Hash, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum Delta {
    Append(Vec<u8>),
    Truncate(usize),
    Reset,
}

#[derive(Default)]
pub struct Deltas {
    tree: BTreeMap<SequenceNumber, Delta>,
}

pub fn merge<'a>(deltas: impl Iterator<Item = &'a Delta>) -> Delta {
    Delta::Append(resolve(deltas))
}

pub fn resolve<'a>(deltas: impl Iterator<Item = &'a Delta>) -> Vec<u8> {
    let mut buffer = Vec::<u8>::new();
    for delta in deltas {
        match delta {
            Delta::Append(bytes) => buffer.extend(bytes),
            Delta::Truncate(n) => buffer.truncate(*n),
            Delta::Reset => buffer.clear(),
        };
    }
    buffer
}

impl Deltas {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn resolve(&self) -> Vec<u8> {
        resolve(self.tree.iter().map(|(_, delta)| delta))
    }

    pub fn since(&self, seqn: &SequenceNumber) -> Vec<Delta> {
        self.tree
            .range((seqn + 1)..)
            .map(|(_, delta)| delta)
            .cloned()
            .collect()
    }

    pub fn latest_seqn(&self) -> SequenceNumber {
        *self.tree.keys().rev().next().unwrap_or(&0)
    }

    pub fn insert(&mut self, seq: SequenceNumber, delta: Delta) {
        self.tree.insert(seq, delta);
    }

    pub fn insert_many(&mut self, deltas: impl IntoIterator<Item = (SequenceNumber, Delta)>) {
        for (seq, delta) in deltas.into_iter() {
            self.tree.insert(seq, delta);
        }
    }

    pub fn checksum(&self) -> u32 {
        let mut hasher = crc32fast::Hasher::new();
        self.tree.hash(&mut hasher);
        hasher.finalize()
    }
}

impl std::fmt::Debug for Deltas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.resolve())
    }
}

impl From<Vec<(SequenceNumber, Delta)>> for Deltas {
    fn from(deltas: Vec<(SequenceNumber, Delta)>) -> Self {
        let mut this = Self::default();
        for (seq, delta) in deltas {
            this.insert(seq, delta);
        }
        this
    }
}

#[cfg(test)]
mod tests {
    use super::Delta;
    use super::Deltas;

    #[test]
    fn resolve() {
        let deltas = [
            (0, Delta::Append(vec![1, 2, 3])), // [ 1, 2, 3 ]
            (1, Delta::Reset),                 // [ ]
            (2, Delta::Append(vec![4, 5])),    // [ 4, 5 ]
            (3, Delta::Truncate(1)),           // [ 4 ]
            (4, Delta::Append(vec![6, 7, 8])), // [ 4, 6, 7, 8 ]
            (5, Delta::Truncate(3)),           // [ 4, 6, 7 ]
        ];
        let deltas = Deltas::from(deltas.to_vec());
        assert_eq!(deltas.resolve(), vec![4, 6, 7]);
    }
}
