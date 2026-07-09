use crate::io::formats::mol2::{Mol2Atom, Mol2Bond, Mol2Crysin};

use super::reticular::SlfReticular;
use super::sectioned::SlfExtensionBlock;

#[derive(Debug, Clone)]
pub struct SlfDocument {
    pub title: String,
    pub atoms: Vec<Mol2Atom>,
    pub bonds: Vec<Mol2Bond>,
    pub crysin: Option<Mol2Crysin>,
    pub extensions: Vec<SlfExtensionBlock>,
    pub reticular: Option<SlfReticular>,
}

impl SlfDocument {
    pub(crate) fn extension_block(&self, name: &str) -> Option<&SlfExtensionBlock> {
        self.extensions
            .iter()
            .find(|block| block.name.eq_ignore_ascii_case(name))
    }
}
