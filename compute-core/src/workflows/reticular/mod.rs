mod builder;
mod library;
mod recipe;

pub use builder::build_framework;
pub use library::{
    component_label, component_template, core_options_for, functional_group_options,
    linker_options, network_options, network_template,
};
#[allow(unused_imports)]
pub use recipe::{
    ComponentSource, CoreSlot, FunctionalizationRule, LinkerDirection, NetworkId,
    ReticularBuildSpec, StackingMode,
};
