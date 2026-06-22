//! Integrate a user-supplied GROMACS `.itp` force-field fragment with the
//! framework topology generator.
//!
//! The framework builder needs to know which atom types a custom force field
//! defines so it can (a) accept elements the built-in tables lack and (b) avoid
//! emitting a duplicate `[atomtypes]` entry for a type the user overrides. This
//! module parses just the `[ atomtypes ]` section(s) of the fragment to collect
//! those names; the rest of the file is passed through to grompp verbatim via an
//! `#include`.

use std::collections::BTreeSet;

use crate::md::CustomTypes;

/// Build the engine-neutral [`CustomTypes`] a framework build needs from a raw
/// GROMACS `.itp` fragment.
pub fn custom_types(itp: &str) -> CustomTypes {
    CustomTypes {
        names: defined_atom_types(itp),
    }
}

/// The atom-type names a GROMACS `.itp` fragment defines, taken as the first
/// whitespace token of each row inside a `[ atomtypes ]` section. Comments
/// (`;`-introduced) and preprocessor lines (`#include`, `#define`) are skipped.
pub fn defined_atom_types(itp: &str) -> BTreeSet<String> {
    let mut types = BTreeSet::new();
    let mut in_atomtypes = false;
    for raw in itp.lines() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(section) = section_name(line) {
            in_atomtypes = section.eq_ignore_ascii_case("atomtypes");
            continue;
        }
        if line.starts_with('#') {
            // A preprocessor directive (e.g. #include) is not an atom-type row.
            continue;
        }
        if in_atomtypes && let Some(name) = line.split_whitespace().next() {
            types.insert(name.to_string());
        }
    }
    types
}

/// Drop an inline `;` comment from a line.
fn strip_comment(line: &str) -> &str {
    match line.find(';') {
        Some(idx) => &line[..idx],
        None => line,
    }
}

/// The directive name of a `[ section ]` header line, or `None` if `line` is not
/// a section header.
fn section_name(line: &str) -> Option<&str> {
    let inside = line.strip_prefix('[')?.strip_suffix(']')?;
    Some(inside.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
; a custom force field
[ atomtypes ]
; name  at.num  mass    charge  ptype  sigma   epsilon
Pt      78      195.08  0.0     A      0.2754  0.3300
Ni      28      58.69   0.0     A      0.2520  0.2100  ; inline comment
[ bondtypes ]
Pt  Pt  1  0.270  200000.0
";

    #[test]
    fn collects_atomtype_names_only() {
        let types = defined_atom_types(SAMPLE);
        assert!(types.contains("Pt"));
        assert!(types.contains("Ni"));
        // Names from the [bondtypes] section are not atom types.
        assert!(!types.contains("0.270"));
        assert_eq!(types.len(), 2);
    }

    #[test]
    fn custom_types_wraps_the_names() {
        let custom = custom_types(SAMPLE);
        assert!(custom.contains("Pt"));
        assert!(!custom.is_empty());
    }

    #[test]
    fn preprocessor_and_blank_lines_are_ignored() {
        let itp = "[ atomtypes ]\n#define FOO 1\n\nAu 79 196.97 0 A 0.26 0.15\n";
        let types = defined_atom_types(itp);
        assert_eq!(types.len(), 1);
        assert!(types.contains("Au"));
    }
}
