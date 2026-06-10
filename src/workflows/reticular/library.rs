use std::collections::HashSet;

use nalgebra::{Point3, Vector3};

use super::recipe::{ComponentSource, NetworkId};
use crate::io::formats::psf::{PsfDocument, PsfReticular, parse_psf_document};

#[derive(Debug, Clone)]
pub struct ComponentTemplate {
    pub label: String,
    pub class: ComponentClass,
    pub connectivity: usize,
    pub atoms: Vec<TemplateAtom>,
    pub bonds: Vec<TemplateBond>,
    pub coordination_sites: Vec<CoordinationSite>,
}

#[derive(Debug, Clone)]
pub struct TemplateBond {
    pub a: usize,
    pub b: usize,
    pub bond_type: crate::domain::BondType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentClass {
    Core,
    Linker,
    FunctionalGroup,
}

#[derive(Debug, Clone)]
pub struct TemplateAtom {
    pub element: String,
    pub position: Point3<f32>,
}

#[derive(Debug, Clone)]
pub struct CoordinationSite {
    pub binding_atom: usize,
    pub coordination_position: Vector3<f32>,
}

#[derive(Debug, Clone)]
pub struct NetworkTemplate {
    pub primary_label: &'static str,
    pub primary_connectivity: usize,
    pub secondary_label: Option<&'static str>,
    pub secondary_connectivity: Option<usize>,
}

/// Represents a builtin component loaded from embedded structure files
#[derive(Debug, Clone)]
pub struct BuiltinComponent {
    pub label: &'static str,
    pub connectivity: usize,
    data: &'static str,
}

impl BuiltinComponent {
    const fn new(label: &'static str, connectivity: usize, data: &'static str) -> Self {
        Self {
            label,
            connectivity,
            data,
        }
    }

    pub fn template(&self) -> ComponentTemplate {
        parse_component_psf(self.data)
    }
}

macro_rules! register_core {
    ($label:expr, $conn:expr, $name:expr) => {
        BuiltinComponent::new(
            $label,
            $conn,
            include_str!(concat!("building_block/core/", $name, ".slf")),
        )
    };
}

macro_rules! register_linker {
    ($label:expr, $name:expr) => {
        BuiltinComponent::new(
            $label,
            2,
            include_str!(concat!("building_block/linker/", $name, ".slf")),
        )
    };
}

macro_rules! register_fg {
    ($label:expr, $name:expr) => {
        BuiltinComponent::new(
            $label,
            1,
            include_str!(concat!("building_block/functional_group/", $name, ".slf")),
        )
    };
}

pub const CORE_COMPONENTS: &[BuiltinComponent] = &[
    register_core!("BENZ", 3, "t3/benzene"),
    register_core!("TRZN", 3, "t3/trzn"),
    register_core!("PYRI", 3, "t3/pyridinium"),
];

pub const LINKER_COMPONENTS: &[BuiltinComponent] = &[
    register_linker!("trans C=C", "trans_ethene"),
    register_linker!("Ph-L2", "benzene"),
];

pub const FUNCTIONAL_GROUP_COMPONENTS: &[BuiltinComponent] = &[
    register_fg!("F", "fluorine"),
    register_fg!("Cl", "chlorine"),
    register_fg!("Br", "bromine"),
    register_fg!("I", "iodine"),
    register_fg!("OH", "hydroxyl"),
    register_fg!("CH3", "methyl"),
    register_fg!("CHO", "aldehyde"),
    register_fg!("COOH", "carboxyl"),
    register_fg!("NH2", "amino"),
    register_fg!("NO2", "nitro"),
    register_fg!("OCH3", "methoxy"),
    register_fg!("OCH2CH3", "ethoxy"),
    register_fg!("C=O", "carbonyl"),
    register_fg!("tBu", "tert_butyl"),
    register_fg!("Ph", "phenyl"),
    register_fg!("CN", "cyano"),
    register_fg!("SH", "thiol"),
    register_fg!("CF3", "trifluoromethyl"),
    register_fg!("SO3H", "sulfonic_acid"),
    register_fg!("CH2CH2CH2SO3-", "sulfonatopropyl"),
    register_fg!("SO2H", "sulfinic_acid"),
    register_fg!("OCOCH3", "acetoxy"),
    register_fg!("OCH2CH2CH3", "propoxy"),
    register_fg!("NO", "nitroso"),
    register_fg!("CHS", "thial"),
    register_fg!("OCH(CH3)2", "isopropoxy"),
    register_fg!("CH=CH2", "vinyl"),
    register_fg!("COCH3", "acetyl"),
    register_fg!("SCH3", "methylthio"),
    register_fg!("N(CH3)2", "dimethylamino"),
];

pub fn core_options_for(connectivity: usize, custom_components: &[String]) -> Vec<ComponentSource> {
    let mut options: Vec<ComponentSource> = CORE_COMPONENTS
        .iter()
        .enumerate()
        .filter(|(_, comp)| comp.connectivity == connectivity)
        .map(|(idx, _)| ComponentSource::BuiltinCore(idx))
        .collect();

    options.extend(custom_component_options(
        custom_components,
        ComponentClass::Core,
        Some(connectivity),
    ));
    options
}

pub fn linker_options(custom_components: &[String]) -> Vec<Option<ComponentSource>> {
    let mut options: Vec<Option<ComponentSource>> = std::iter::once(None)
        .chain(
            LINKER_COMPONENTS
                .iter()
                .enumerate()
                .map(|(idx, _)| Some(ComponentSource::BuiltinLinker(idx))),
        )
        .collect();

    options.extend(
        custom_component_options(custom_components, ComponentClass::Linker, None)
            .into_iter()
            .map(Some),
    );
    options
}

pub fn functional_group_options(custom_components: &[String]) -> Vec<Option<ComponentSource>> {
    let mut options: Vec<Option<ComponentSource>> = std::iter::once(None)
        .chain(
            FUNCTIONAL_GROUP_COMPONENTS
                .iter()
                .enumerate()
                .map(|(idx, _)| Some(ComponentSource::BuiltinFunctionalGroup(idx))),
        )
        .collect();

    options.extend(
        custom_component_options(custom_components, ComponentClass::FunctionalGroup, None)
            .into_iter()
            .map(Some),
    );
    options
}

pub fn network_options() -> &'static [NetworkId] {
    &[NetworkId::HoneycombVertexVertex]
}

pub fn component_template(
    source: ComponentSource,
    custom_components: &[String],
) -> ComponentTemplate {
    match source {
        ComponentSource::BuiltinCore(idx) => CORE_COMPONENTS[idx].template(),
        ComponentSource::BuiltinLinker(idx) => LINKER_COMPONENTS[idx].template(),
        ComponentSource::BuiltinFunctionalGroup(idx) => FUNCTIONAL_GROUP_COMPONENTS[idx].template(),
        ComponentSource::Custom(index) => parse_component_psf(&custom_components[index]),
    }
}

pub fn component_label(source: ComponentSource, custom_components: &[String]) -> String {
    match source {
        ComponentSource::BuiltinCore(idx) => CORE_COMPONENTS[idx].label.to_string(),
        ComponentSource::BuiltinLinker(idx) => LINKER_COMPONENTS[idx].label.to_string(),
        ComponentSource::BuiltinFunctionalGroup(idx) => {
            FUNCTIONAL_GROUP_COMPONENTS[idx].label.to_string()
        }
        ComponentSource::Custom(index) => parse_component_psf(&custom_components[index]).label,
    }
}

pub fn network_template(id: NetworkId) -> NetworkTemplate {
    match id {
        NetworkId::HoneycombVertexVertex => NetworkTemplate {
            primary_label: "Core A",
            primary_connectivity: 3,
            secondary_label: Some("Core B"),
            secondary_connectivity: Some(3),
        },
    }
}

fn custom_component_options(
    custom_components: &[String],
    class: ComponentClass,
    connectivity: Option<usize>,
) -> Vec<ComponentSource> {
    custom_components
        .iter()
        .enumerate()
        .filter_map(|(index, source)| {
            let component = parse_component_psf(source);
            (component.class == class
                && connectivity.is_none_or(|required| component.connectivity == required))
            .then_some(ComponentSource::Custom(index))
        })
        .collect()
}

fn parse_component_psf(input: &str) -> ComponentTemplate {
    let document = parse_psf_document(input).expect("embedded PSF component must parse");
    let reticular = document
        .reticular
        .clone()
        .expect("embedded PSF component must include reticular metadata");
    parse_psf_component(document, reticular)
}

fn parse_psf_component(document: PsfDocument, reticular: PsfReticular) -> ComponentTemplate {
    let class = match reticular.class.as_str() {
        "core" => ComponentClass::Core,
        "linker" => ComponentClass::Linker,
        "functional_group" => ComponentClass::FunctionalGroup,
        _ => ComponentClass::Core,
    };
    let leaving_atoms = reticular
        .substitution_sites
        .iter()
        .map(|site| site.leaving_atom)
        .collect::<HashSet<_>>();
    let mut atom_map = vec![None; document.atoms.len()];
    let mut atoms = Vec::new();

    for (old_index, atom) in document.atoms.iter().enumerate() {
        if leaving_atoms.contains(&old_index) {
            continue;
        }

        atom_map[old_index] = Some(atoms.len());
        atoms.push(TemplateAtom {
            element: atom.element.clone(),
            position: atom.position,
        });
    }

    let bonds = document
        .bonds
        .iter()
        .filter_map(|bond| {
            Some(TemplateBond {
                a: atom_map[bond.a]?,
                b: atom_map[bond.b]?,
                bond_type: bond.bond_type,
            })
        })
        .collect::<Vec<_>>();
    let coordination_sites = reticular
        .substitution_sites
        .iter()
        .filter_map(|site| {
            Some(CoordinationSite {
                binding_atom: atom_map[site.binding_atom]?,
                coordination_position: document.atoms.get(site.leaving_atom)?.position.coords,
            })
        })
        .collect::<Vec<_>>();
    ComponentTemplate {
        label: reticular.label.unwrap_or(document.title),
        class,
        connectivity: coordination_sites.len(),
        atoms,
        bonds,
        coordination_sites,
    }
}

#[cfg(test)]
mod tests {
    use super::{CORE_COMPONENTS, ComponentClass, FUNCTIONAL_GROUP_COMPONENTS, LINKER_COMPONENTS};

    #[test]
    fn builtin_psf_metadata_is_valid() {
        for component in CORE_COMPONENTS {
            validate_builtin_component(
                component.label,
                component.data,
                ComponentClass::Core,
                component.connectivity,
            );
        }
        for component in LINKER_COMPONENTS {
            validate_builtin_component(
                component.label,
                component.data,
                ComponentClass::Linker,
                component.connectivity,
            );
        }
        for component in FUNCTIONAL_GROUP_COMPONENTS {
            validate_builtin_component(
                component.label,
                component.data,
                ComponentClass::FunctionalGroup,
                component.connectivity,
            );
        }
    }

    fn validate_builtin_component(
        label: &str,
        data: &str,
        expected_class: ComponentClass,
        expected_connectivity: usize,
    ) {
        let document = crate::io::formats::psf::parse_psf_document(data)
            .unwrap_or_else(|error| panic!("{label}: invalid PSF: {error}"));
        let reticular = document
            .reticular
            .as_ref()
            .unwrap_or_else(|| panic!("{label}: missing reticular metadata"));
        let actual_class = match reticular.class.as_str() {
            "core" => ComponentClass::Core,
            "linker" => ComponentClass::Linker,
            "functional_group" => ComponentClass::FunctionalGroup,
            other => panic!("{label}: unknown reticular class {other}"),
        };

        assert_eq!(
            actual_class, expected_class,
            "{label}: reticular class mismatch"
        );
        assert_eq!(
            reticular.substitution_sites.len(),
            expected_connectivity,
            "{label}: wrong number of substitution sites"
        );

        for site in &reticular.substitution_sites {
            assert!(
                site.leaving_atom < document.atoms.len(),
                "{label}: substitution leaving atom {} is out of range",
                site.leaving_atom + 1
            );
            assert!(
                site.binding_atom < document.atoms.len(),
                "{label}: substitution binding atom {} is out of range",
                site.binding_atom + 1
            );
            assert_ne!(
                site.leaving_atom,
                site.binding_atom,
                "{label}: substitution site uses the same atom {} as leaving and binding atom",
                site.leaving_atom + 1
            );

            let leaving = &document.atoms[site.leaving_atom];
            let binding = &document.atoms[site.binding_atom];
            assert_eq!(
                leaving.element,
                "Du",
                "{label}: leaving atom {} must be Du, got {}",
                site.leaving_atom + 1,
                leaving.element
            );
            assert_ne!(
                binding.element,
                "Du",
                "{label}: binding atom {} must not be Du",
                site.binding_atom + 1
            );
            assert!(
                document.bonds.iter().any(|bond| {
                    (bond.a == site.leaving_atom && bond.b == site.binding_atom)
                        || (bond.a == site.binding_atom && bond.b == site.leaving_atom)
                }),
                "{label}: substitution site {} -> {} is not backed by a PSF bond",
                site.leaving_atom + 1,
                site.binding_atom + 1
            );
        }

        let template = super::parse_component_psf(data);
        assert_eq!(
            template.class, expected_class,
            "{label}: parsed template class mismatch"
        );
        assert_eq!(
            template.connectivity, expected_connectivity,
            "{label}: parsed template connectivity mismatch"
        );
    }
}
