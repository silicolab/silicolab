//! Build-time input describing a post-translational modification to attach to a
//! protein, mirroring how `GlycosylationKind` parameterises glycan attachment.
//! The fragment geometry and topology for each kind live with the PTM builders;
//! this module only fixes the vocabulary they share.

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtmKind {
    Phosphoryl,
    Acetyl { n_terminal: bool },
    Methyl { degree: MethylDegree },
    Acyl(AcylKind),
    Prenyl(PrenylKind),
    Ubl(UblKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethylDegree {
    Mono,
    Di,
    Tri,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcylKind {
    Palmitoyl,
    Myristoyl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrenylKind {
    Farnesyl,
    GeranylGeranyl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UblKind {
    Ubiquitin,
    Sumo,
    Nedd8,
}
