use anyhow::{Context, Result, anyhow};

use super::sectioned::{
    PsfExtensionBlock, PsfExtensionPayload, PsfTableBlock, required_column_index,
};

#[derive(Debug, Clone)]
pub struct PsfReticular {
    pub class: String,
    pub label: Option<String>,
    pub substitution_sites: Vec<PsfSubstitutionSite>,
}

#[derive(Debug, Clone, Copy)]
pub struct PsfSubstitutionSite {
    pub leaving_atom: usize,
    pub binding_atom: usize,
}

pub(crate) fn parse_reticular_block(block: &PsfExtensionBlock) -> Result<PsfReticular> {
    let PsfExtensionPayload::Sectioned(sectioned) = &block.payload;
    let table = sectioned
        .first_table()
        .ok_or_else(|| anyhow!("@<SILICOLAB>RETICULAR requires a #TABLE section"))?;

    Ok(PsfReticular {
        class: sectioned.key_value("class").unwrap_or("core").to_string(),
        label: sectioned.key_value("label").map(str::to_string),
        substitution_sites: parse_reticular_sites_from_table(table)?,
    })
}

pub(crate) fn serialize_reticular_block(reticular: &PsfReticular) -> String {
    let mut output = String::new();
    output.push_str("@<SILICOLAB>RETICULAR\n");
    output.push_str("#KEY\n");
    output.push_str(&format!("class {}\n", reticular.class));
    if let Some(label) = &reticular.label {
        output.push_str(&format!("label {}\n", label));
    }
    output.push_str("\n#TABLE\n");
    output.push_str("leaving_atom binding_atom\n");
    for site in &reticular.substitution_sites {
        output.push_str(&format!(
            "{} {}\n",
            site.leaving_atom + 1,
            site.binding_atom + 1
        ));
    }
    output
}

fn parse_reticular_sites_from_table(table: &PsfTableBlock) -> Result<Vec<PsfSubstitutionSite>> {
    let leaving_atom_index = required_column_index(table, "leaving_atom")?;
    let binding_atom_index = required_column_index(table, "binding_atom")?;
    let mut substitution_sites = Vec::new();

    for row in &table.rows {
        let leaving_atom = row[leaving_atom_index]
            .parse::<usize>()
            .context("invalid reticular leaving_atom")?;
        let binding_atom = row[binding_atom_index]
            .parse::<usize>()
            .context("invalid reticular binding_atom")?;

        substitution_sites.push(PsfSubstitutionSite {
            leaving_atom: leaving_atom - 1,
            binding_atom: binding_atom - 1,
        });
    }

    Ok(substitution_sites)
}
