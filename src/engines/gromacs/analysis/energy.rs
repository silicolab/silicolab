//! Stdin selection builders for interactive GROMACS analysis tools.
//!
//! `gmx energy` (and several other tools) print a numbered menu of available
//! terms and read the user's choices from stdin. In the tutorial this is driven
//! with `printf "Potential\n0\n" | gmx energy ...`: each desired term on its own
//! line, followed by `0` (or a blank line) to end the selection. These helpers
//! build that byte payload so it can be handed to
//! [`crate::engines::process::ProcessConfig::stdin_bytes`].

/// Build the stdin payload selecting the given energy terms for `gmx energy`.
///
/// Each term is emitted on its own line and the list is terminated with `0`,
/// matching the tutorial's `printf "Potential\n0\n"` idiom. Terms may be names
/// (`Potential`, `Temperature`, `Pressure`, `Density`) or the numeric indices
/// from the tool's menu.
pub fn energy_term_selection(terms: &[&str]) -> Vec<u8> {
    let mut payload = String::new();
    for term in terms {
        payload.push_str(term);
        payload.push('\n');
    }
    payload.push_str("0\n");
    payload.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_term_matches_tutorial_idiom() {
        assert_eq!(energy_term_selection(&["Potential"]), b"Potential\n0\n");
    }

    #[test]
    fn multiple_terms_are_newline_separated() {
        assert_eq!(
            energy_term_selection(&["Temperature", "Pressure"]),
            b"Temperature\nPressure\n0\n"
        );
    }
}
