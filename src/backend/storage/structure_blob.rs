use std::collections::HashMap;

use anyhow::Result;
use rusqlite::{Connection, params};

use crate::{
    backend::structure_codec::{PAYLOAD_FORMAT, decode_structure, encode_structure},
    domain::Structure,
};

pub(crate) fn load_compound_revisions(db: &Connection) -> Result<HashMap<i64, i64>> {
    let mut statement = db.prepare("select id, revision from compounds")?;
    let rows = statement.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?;
    let mut map = HashMap::new();
    for row in rows {
        let (id, revision) = row?;
        map.insert(id, revision);
    }
    Ok(map)
}

pub(crate) fn save_structure(
    db: &Connection,
    compound_id: i64,
    revision: i64,
    structure: &Structure,
) -> Result<()> {
    let blob = encode_structure(structure)?;
    let kind = if structure.biopolymer.is_some() {
        "biopolymer"
    } else if structure.cell.is_some() {
        "periodic"
    } else {
        "structure"
    };
    db.execute(
        "insert or replace into compounds (id, title, kind, atom_count, bond_count, revision, format, payload, uncompressed_len) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            compound_id,
            structure.title,
            kind,
            structure.atoms.len() as i64,
            structure.bonds.len() as i64,
            revision,
            PAYLOAD_FORMAT,
            blob.bytes,
            blob.uncompressed_len as i64,
        ],
    )?;
    Ok(())
}

pub(crate) fn load_structure(db: &Connection, compound_id: i64) -> Result<Structure> {
    let (payload, uncompressed_len) = db.query_row(
        "select payload, uncompressed_len from compounds where id = ?1",
        params![compound_id],
        |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)? as usize)),
    )?;
    decode_structure(&payload, uncompressed_len)
}
