use crate::io::formats::mol2::{Mol2Atom, Mol2Bond, Mol2Crysin};

use super::reticular::PsfReticular;
use super::sectioned::PsfExtensionBlock;

#[derive(Debug, Clone)]
pub struct PsfDocument {
    pub title: String,
    pub atoms: Vec<Mol2Atom>,
    pub bonds: Vec<Mol2Bond>,
    pub crysin: Option<Mol2Crysin>,
    pub extensions: Vec<PsfExtensionBlock>,
    pub reticular: Option<PsfReticular>,
}

impl PsfDocument {
    pub(crate) fn extension_block(&self, name: &str) -> Option<&PsfExtensionBlock> {
        self.extensions
            .iter()
            .find(|block| block.name.eq_ignore_ascii_case(name))
    }
}
