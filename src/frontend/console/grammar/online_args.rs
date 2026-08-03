use anyhow::{Result, anyhow};
use clap::{FromArgMatches, ValueEnum};

use super::{Command, Root, root_command};

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum FindQueryKind {
    Auto,
    Name,
    Formula,
    Smiles,
}

impl From<FindQueryKind> for crate::io::online_structures::QueryKind {
    fn from(value: FindQueryKind) -> Self {
        match value {
            FindQueryKind::Auto => Self::Auto,
            FindQueryKind::Name => Self::Name,
            FindQueryKind::Formula => Self::Formula,
            FindQueryKind::Smiles => Self::Smiles,
        }
    }
}

pub(super) fn parse_find_limit(value: &str) -> std::result::Result<usize, String> {
    let limit = value
        .parse::<usize>()
        .map_err(|_| "limit must be an integer from 1 to 20".to_string())?;
    if (1..=20).contains(&limit) {
        Ok(limit)
    } else {
        Err("limit must be from 1 to 20".to_string())
    }
}

pub(crate) fn parse_find_query(
    command: &str,
) -> Result<Option<crate::io::online_structures::StructureQuery>> {
    let words = super::super::shell_words(command)?;
    if words.first().is_none_or(|word| word != "find") {
        return Ok(None);
    }
    let matches = root_command()
        .try_get_matches_from(&words)
        .map_err(|error| anyhow!("{error}"))?;
    match Root::from_arg_matches(&matches)
        .map_err(|error| anyhow!("{error}"))?
        .command
    {
        Command::Find {
            query,
            kind,
            include_disorder,
            limit,
        } => {
            let mut value = crate::io::online_structures::StructureQuery::new(query, kind.into());
            value.include_disorder = include_disorder;
            value.limit = limit;
            value.validate()?;
            Ok(Some(value))
        }
        _ => Ok(None),
    }
}
