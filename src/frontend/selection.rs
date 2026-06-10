use std::collections::BTreeSet;

#[derive(Debug, Clone, Default)]
pub struct AtomSelection {
    atoms: BTreeSet<usize>,
    primary: Option<usize>,
}

impl AtomSelection {
    /// Rebuild a selection from its persisted parts (selected atom indices and
    /// the primary atom). The primary is only retained if it is part of the set.
    pub fn from_parts(atoms: impl IntoIterator<Item = usize>, primary: Option<usize>) -> Self {
        let atoms: BTreeSet<usize> = atoms.into_iter().collect();
        let primary = primary.filter(|index| atoms.contains(index));
        Self { atoms, primary }
    }

    pub fn clear(&mut self) {
        self.atoms.clear();
        self.primary = None;
    }

    pub fn is_empty(&self) -> bool {
        self.atoms.is_empty()
    }

    pub fn len(&self) -> usize {
        self.atoms.len()
    }

    pub fn primary(&self) -> Option<usize> {
        self.primary.filter(|index| self.atoms.contains(index))
    }

    pub fn contains(&self, index: usize) -> bool {
        self.atoms.contains(&index)
    }

    pub fn ordered_indices(&self) -> Vec<usize> {
        self.atoms.iter().copied().collect()
    }

    pub fn select_only(&mut self, index: usize) {
        self.atoms.clear();
        self.atoms.insert(index);
        self.primary = Some(index);
    }

    pub fn add(&mut self, index: usize) {
        self.atoms.insert(index);
        self.primary = Some(index);
    }

    pub fn remove(&mut self, index: usize) {
        self.atoms.remove(&index);
        if self.primary == Some(index) {
            self.primary = self.atoms.iter().next_back().copied();
        }
    }

    pub fn toggle(&mut self, index: usize) {
        if self.contains(index) {
            self.remove(index);
        } else {
            self.add(index);
        }
    }

    pub fn select_all(&mut self, atom_count: usize) {
        self.atoms = (0..atom_count).collect();
        self.primary = (!self.atoms.is_empty()).then_some(0);
    }

    /// Replace the selection with exactly the given atom indices.
    pub fn select_indices(&mut self, indices: impl IntoIterator<Item = usize>) {
        self.atoms = indices.into_iter().collect();
        self.primary = self.atoms.iter().next().copied();
    }

    pub fn invert(&mut self, atom_count: usize) {
        let inverted = (0..atom_count)
            .filter(|index| !self.atoms.contains(index))
            .collect::<BTreeSet<_>>();
        self.atoms = inverted;
        self.primary = self
            .primary
            .filter(|index| self.atoms.contains(index))
            .or_else(|| self.atoms.iter().next().copied());
    }

    pub fn retain_valid(&mut self, atom_count: usize) {
        self.atoms.retain(|index| *index < atom_count);
        self.primary = self
            .primary
            .filter(|index| self.atoms.contains(index))
            .or_else(|| self.atoms.iter().next().copied());
    }
}
