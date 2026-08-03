use std::{
    cmp::Ordering,
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::{domain::Structure, io::formats::cif::parse_cif};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_JSON_BYTES: u64 = 16 * 1024 * 1024;
const MAX_CIF_BYTES: u64 = 128 * 1024 * 1024;
const MAX_COD_MATCHES: usize = 250;
const DEFAULT_LIMIT: usize = 20;
const USER_AGENT: &str = concat!("silicolab/", env!("CARGO_PKG_VERSION"));

pub const PUBCHEM_DEFAULT_BASE_URL: &str = "https://pubchem.ncbi.nlm.nih.gov/rest/pug";
pub const COD_DEFAULT_BASE_URL: &str = "https://www.crystallography.net/cod";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryKind {
    Auto,
    Name,
    Formula,
    Smiles,
}

impl QueryKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Name => "Name",
            Self::Formula => "Formula",
            Self::Smiles => "SMILES",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructureQuery {
    pub text: String,
    pub kind: QueryKind,
    pub include_disorder: bool,
    pub limit: usize,
}

impl StructureQuery {
    pub fn new(text: impl Into<String>, kind: QueryKind) -> Self {
        Self {
            text: text.into(),
            kind,
            include_disorder: false,
            limit: DEFAULT_LIMIT,
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.text.trim().is_empty() {
            bail!("a structure name, formula, or SMILES is required");
        }
        if !(1..=DEFAULT_LIMIT).contains(&self.limit) {
            bail!("result limit must be between 1 and {DEFAULT_LIMIT}");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedCompound {
    pub cid: u64,
    pub title: String,
    pub formula: String,
    pub smiles: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrystalCandidate {
    pub cod_id: String,
    pub revision: Option<String>,
    pub cid: Option<u64>,
    pub smiles: Option<String>,
    pub name: String,
    pub formula: String,
    pub temperature_k: Option<f64>,
    pub space_group: Option<String>,
    pub r_factor: Option<f64>,
    pub doi: Option<String>,
    pub flags: Vec<String>,
    pub exact_formula: bool,
    pub warnings: Vec<String>,
}

impl CrystalCandidate {
    pub fn download_url(&self, base_url: &str) -> String {
        let base = base_url.trim_end_matches('/');
        match self.revision.as_deref() {
            Some(revision) if !revision.is_empty() => {
                format!("{base}/{}.cif@{revision}", self.cod_id)
            }
            _ => format!("{base}/{}.cif", self.cod_id),
        }
    }

    pub fn is_importable(&self) -> bool {
        self.revision
            .as_deref()
            .is_some_and(|value| !value.is_empty())
            && !self
                .flags
                .iter()
                .any(|flag| flag.to_ascii_lowercase().contains("disorder"))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructureSearchResult {
    pub query: String,
    pub resolved: Option<ResolvedCompound>,
    pub crystals: Vec<CrystalCandidate>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnlineProvider {
    Cod,
    Pubchem,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnlineStructureSource {
    pub provider: OnlineProvider,
    pub record_id: String,
    pub query: String,
    pub revision: Option<String>,
    pub source_url: String,
    pub cid: Option<u64>,
    pub smiles: Option<String>,
    pub formula: Option<String>,
    pub temperature_k: Option<String>,
    pub space_group: Option<String>,
    pub r_factor: Option<String>,
    pub doi: Option<String>,
    pub flags: Vec<String>,
    pub retrieved_at_ms: u64,
}

impl OnlineStructureSource {
    pub fn from_cod(query: &str, candidate: &CrystalCandidate, base_url: &str) -> Self {
        Self {
            provider: OnlineProvider::Cod,
            record_id: candidate.cod_id.clone(),
            query: query.to_string(),
            revision: candidate.revision.clone(),
            source_url: candidate.download_url(base_url),
            cid: candidate.cid,
            smiles: candidate.smiles.clone(),
            formula: nonempty(&candidate.formula),
            temperature_k: candidate.temperature_k.map(|value| value.to_string()),
            space_group: candidate.space_group.clone(),
            r_factor: candidate.r_factor.map(|value| value.to_string()),
            doi: candidate.doi.clone(),
            flags: candidate
                .flags
                .iter()
                .chain(&candidate.warnings)
                .cloned()
                .collect(),
            retrieved_at_ms: now_ms(),
        }
    }

    pub fn from_pubchem(query: &str, compound: &ResolvedCompound) -> Self {
        Self {
            provider: OnlineProvider::Pubchem,
            record_id: compound.cid.to_string(),
            query: query.to_string(),
            revision: None,
            source_url: format!("https://pubchem.ncbi.nlm.nih.gov/compound/{}", compound.cid),
            cid: Some(compound.cid),
            smiles: Some(compound.smiles.clone()),
            formula: Some(compound.formula.clone()),
            temperature_k: None,
            space_group: None,
            r_factor: None,
            doi: None,
            flags: vec!["generated from SMILES; not an experimental crystal".to_string()],
            retrieved_at_ms: now_ms(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SearchEndpoints {
    pub pubchem_base_url: String,
    pub cod_base_url: String,
}

impl Default for SearchEndpoints {
    fn default() -> Self {
        Self {
            pubchem_base_url: PUBCHEM_DEFAULT_BASE_URL.to_string(),
            cod_base_url: COD_DEFAULT_BASE_URL.to_string(),
        }
    }
}

#[derive(Debug)]
pub struct FetchedCod {
    pub path: PathBuf,
    pub downloaded: bool,
    pub structure: Structure,
    pub source: OnlineStructureSource,
}

pub fn search_structures(query: &StructureQuery) -> Result<StructureSearchResult> {
    search_structures_with_endpoints(query, &SearchEndpoints::default())
}

pub fn lookup_cod_candidate(id: &str, revision: Option<&str>) -> Result<CrystalCandidate> {
    validate_cod_id(id)?;
    let mut candidates = search_cod("id", id, true, COD_DEFAULT_BASE_URL)?;
    let mut candidate = candidates
        .drain(..)
        .find(|candidate| candidate.cod_id == id)
        .ok_or_else(|| anyhow::anyhow!("COD entry {id} was not found"))?;
    if let Some(revision) = revision {
        if !revision.chars().all(|ch| ch.is_ascii_digit()) {
            bail!("invalid COD revision `{revision}`");
        }
        candidate.revision = Some(revision.to_string());
    }
    Ok(candidate)
}

pub fn search_structures_with_endpoints(
    query: &StructureQuery,
    endpoints: &SearchEndpoints,
) -> Result<StructureSearchResult> {
    query.validate()?;
    let text = query.text.trim();
    let kind = resolved_query_kind(text, query.kind);
    let mut warnings = Vec::new();
    let resolved = match kind {
        QueryKind::Name | QueryKind::Smiles => {
            match resolve_pubchem(text, kind, &endpoints.pubchem_base_url) {
                Ok(value) => value,
                Err(error) if kind == QueryKind::Name => {
                    warnings.push(format!("PubChem could not resolve the name: {error}"));
                    None
                }
                Err(error) => return Err(error),
            }
        }
        QueryKind::Formula | QueryKind::Auto => None,
    };

    let formula = match kind {
        QueryKind::Formula => Some(normalize_formula_for_cod(text)?),
        _ => resolved
            .as_ref()
            .map(|compound| normalize_formula_for_cod(&compound.formula))
            .transpose()?,
    };
    let mut records = Vec::new();
    if let Some(formula) = formula.as_deref() {
        records.extend(search_cod(
            "formula",
            formula,
            true,
            &endpoints.cod_base_url,
        )?);
    }
    if let Some(compound) = resolved.as_ref() {
        match search_cod("smarts", &compound.smiles, false, &endpoints.cod_base_url) {
            Ok(found) => records.extend(found),
            Err(error) => warnings.push(format!("COD structure expansion was skipped: {error}")),
        }
    } else if kind == QueryKind::Name {
        records.extend(search_cod("text", text, false, &endpoints.cod_base_url)?);
    }

    if let Some(compound) = resolved.as_ref() {
        for candidate in &mut records {
            candidate.cid = Some(compound.cid);
            candidate.smiles = Some(compound.smiles.clone());
        }
    }

    let mut seen = HashSet::new();
    records.retain(|candidate| seen.insert(candidate.cod_id.clone()));
    if !query.include_disorder {
        records.retain(|candidate| {
            !candidate
                .flags
                .iter()
                .any(|flag| flag.to_ascii_lowercase().contains("disorder"))
        });
    }
    records.retain(|candidate| !is_rejected_candidate(candidate));
    records.sort_by(compare_candidates);
    records.truncate(query.limit);

    Ok(StructureSearchResult {
        query: text.to_string(),
        resolved,
        crystals: records,
        warnings,
    })
}

pub fn fetch_cod(
    query: &str,
    candidate: &CrystalCandidate,
    structures_dir: &Path,
) -> Result<FetchedCod> {
    fetch_cod_with_base_url(query, candidate, structures_dir, COD_DEFAULT_BASE_URL)
}

pub fn fetch_cod_with_base_url(
    query: &str,
    candidate: &CrystalCandidate,
    structures_dir: &Path,
    base_url: &str,
) -> Result<FetchedCod> {
    validate_cod_id(&candidate.cod_id)?;
    if candidate.revision.as_deref().is_none_or(str::is_empty) {
        bail!(
            "COD {} did not provide a fixed revision and cannot be imported reproducibly",
            candidate.cod_id
        );
    }
    if !candidate.is_importable() {
        bail!(
            "COD {} is marked as disordered and cannot currently be imported",
            candidate.cod_id
        );
    }
    let revision = candidate
        .revision
        .as_deref()
        .filter(|value| !value.is_empty());
    if let Some(revision) = revision
        && !revision.chars().all(|ch| ch.is_ascii_digit())
    {
        bail!("invalid COD revision `{revision}`");
    }
    let file_name = match revision {
        Some(revision) => format!("{}@{revision}.cif", candidate.cod_id),
        None => format!("{}.cif", candidate.cod_id),
    };
    let dir = structures_dir.join("cod");
    let path = dir.join(file_name);
    let source = OnlineStructureSource::from_cod(query, candidate, base_url);
    if path.is_file() {
        let input = fs::read_to_string(&path)
            .with_context(|| format!("failed to read cached {}", path.display()))?;
        let mut structure = parse_cif(&input)
            .with_context(|| format!("cached COD file {} is not usable", path.display()))?;
        structure.title = candidate.name.clone();
        return Ok(FetchedCod {
            path,
            downloaded: false,
            structure,
            source,
        });
    }

    let url = candidate.download_url(base_url);
    let input = get_text(&url, MAX_CIF_BYTES).context("failed to download COD CIF")?;
    if input.trim().is_empty() || looks_like_html(&input) {
        bail!("COD returned a non-CIF response for {}", candidate.cod_id);
    }
    let mut structure = parse_cif(&input)
        .with_context(|| format!("COD {} cannot be imported", candidate.cod_id))?;
    structure.title = candidate.name.clone();
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let temp = dir.join(format!(
        ".{}.{}.tmp",
        candidate.cod_id,
        uuid::Uuid::new_v4()
    ));
    fs::write(&temp, input).with_context(|| format!("failed to write {}", temp.display()))?;
    if let Err(error) = fs::rename(&temp, &path) {
        let _ = fs::remove_file(&temp);
        return Err(error).with_context(|| format!("failed to store {}", path.display()));
    }
    Ok(FetchedCod {
        path,
        downloaded: true,
        structure,
        source,
    })
}

fn resolved_query_kind(text: &str, requested: QueryKind) -> QueryKind {
    if requested != QueryKind::Auto {
        return requested;
    }
    if normalize_formula_for_cod(text).is_ok() {
        QueryKind::Formula
    } else if looks_like_smiles(text) {
        QueryKind::Smiles
    } else {
        QueryKind::Name
    }
}

fn looks_like_smiles(text: &str) -> bool {
    text.chars()
        .any(|ch| matches!(ch, '[' | ']' | '(' | ')' | '=' | '#' | '@' | '/' | '\\'))
        && crate::domain::smiles::parse(text).is_ok()
}

fn normalize_formula_for_cod(input: &str) -> Result<String> {
    let compact = input
        .trim()
        .chars()
        .filter(|ch| !ch.is_ascii_whitespace())
        .collect::<String>();
    if compact.is_empty() {
        bail!("molecular formula is empty");
    }
    let chars = compact.as_bytes();
    let mut index = 0;
    let mut terms = Vec::new();
    while index < chars.len() {
        if !chars[index].is_ascii_uppercase() {
            bail!("`{input}` is not a Hill-style molecular formula");
        }
        let start = index;
        index += 1;
        if index < chars.len() && chars[index].is_ascii_lowercase() {
            index += 1;
        }
        while index < chars.len() && (chars[index].is_ascii_digit() || chars[index] == b'.') {
            index += 1;
        }
        terms.push(compact[start..index].to_string());
    }
    Ok(terms.join(" "))
}

fn resolve_pubchem(
    text: &str,
    kind: QueryKind,
    base_url: &str,
) -> Result<Option<ResolvedCompound>> {
    let namespace = if kind == QueryKind::Smiles {
        "smiles"
    } else {
        "name"
    };
    let encoded = encode_path_segment(text);
    let url = format!(
        "{}/compound/{namespace}/{encoded}/property/Title,MolecularFormula,SMILES/JSON",
        base_url.trim_end_matches('/')
    );
    let body = get_text(&url, MAX_JSON_BYTES).context("PubChem lookup failed")?;
    let response: PubchemResponse =
        serde_json::from_str(&body).context("PubChem returned invalid JSON")?;
    Ok(response
        .property_table
        .properties
        .into_iter()
        .next()
        .map(|property| ResolvedCompound {
            cid: property.cid,
            title: property.title.unwrap_or_else(|| text.to_string()),
            formula: property.molecular_formula,
            smiles: property.smiles,
        }))
}

fn search_cod(
    parameter: &str,
    value: &str,
    exact_formula: bool,
    base_url: &str,
) -> Result<Vec<CrystalCandidate>> {
    let result_url = format!("{}/result", base_url.trim_end_matches('/'));
    let count = get_with_query(&result_url, &[("format", "count"), (parameter, value)])?
        .trim()
        .parse::<usize>()
        .context("COD returned an invalid match count")?;
    if count > MAX_COD_MATCHES {
        bail!(
            "COD matched {count} records, above the {MAX_COD_MATCHES}-record safety cap; refine the query"
        );
    }
    if count == 0 {
        return Ok(Vec::new());
    }
    let body = get_with_query(&result_url, &[("format", "json"), (parameter, value)])?;
    let records: Vec<CodRecord> =
        serde_json::from_str(&body).context("COD returned invalid JSON")?;
    Ok(records
        .into_iter()
        .map(|record| record.into_candidate(exact_formula))
        .collect())
}

fn get_with_query(url: &str, query: &[(&str, &str)]) -> Result<String> {
    let mut request = http_agent().get(url).header("User-Agent", USER_AGENT);
    for (key, value) in query {
        request = request.query(key, value);
    }
    request
        .call()
        .with_context(|| format!("request to {url} failed"))?
        .body_mut()
        .with_config()
        .limit(MAX_JSON_BYTES)
        .read_to_string()
        .with_context(|| format!("failed to read response from {url}"))
}

fn get_text(url: &str, limit: u64) -> Result<String> {
    http_agent()
        .get(url)
        .header("User-Agent", USER_AGENT)
        .call()
        .with_context(|| format!("request to {url} failed"))?
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_string()
        .with_context(|| format!("failed to read response from {url}"))
}

fn http_agent() -> ureq::Agent {
    ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_global(Some(REQUEST_TIMEOUT))
            .build(),
    )
}

fn compare_candidates(left: &CrystalCandidate, right: &CrystalCandidate) -> Ordering {
    candidate_score(right)
        .partial_cmp(&candidate_score(left))
        .unwrap_or(Ordering::Equal)
        .then_with(|| left.cod_id.cmp(&right.cod_id))
}

fn candidate_score(candidate: &CrystalCandidate) -> f64 {
    let formula = if candidate.exact_formula { 1000.0 } else { 0.0 };
    let temperature = candidate
        .temperature_k
        .map(|temperature| 200.0 - (temperature - 295.0).abs().min(200.0))
        .unwrap_or(0.0);
    let refinement = candidate
        .r_factor
        .map(|factor| 100.0 - factor.min(1.0) * 100.0)
        .unwrap_or(0.0);
    formula + temperature + refinement
}

fn is_rejected_candidate(candidate: &CrystalCandidate) -> bool {
    candidate.flags.iter().any(|flag| {
        let lower = flag.to_ascii_lowercase();
        lower.contains("error") || lower.contains("theoretical") || lower.contains("duplicate")
    })
}

fn validate_cod_id(id: &str) -> Result<()> {
    if id.len() != 7 || !id.chars().all(|ch| ch.is_ascii_digit()) {
        bail!("`{id}` is not a valid seven-digit COD id");
    }
    Ok(())
}

fn encode_path_segment(input: &str) -> String {
    let mut output = String::new();
    for byte in input.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_' | b'.' | b'~') {
            output.push(*byte as char);
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

fn nonempty(value: &str) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.to_string())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn looks_like_html(input: &str) -> bool {
    let start = input.trim_start().to_ascii_lowercase();
    start.starts_with("<!doctype html") || start.starts_with("<html")
}

#[derive(Deserialize)]
struct PubchemResponse {
    #[serde(rename = "PropertyTable")]
    property_table: PubchemPropertyTable,
}

#[derive(Deserialize)]
struct PubchemPropertyTable {
    #[serde(rename = "Properties")]
    properties: Vec<PubchemProperty>,
}

#[derive(Deserialize)]
struct PubchemProperty {
    #[serde(rename = "CID")]
    cid: u64,
    #[serde(rename = "Title")]
    title: Option<String>,
    #[serde(rename = "MolecularFormula")]
    molecular_formula: String,
    #[serde(rename = "SMILES", alias = "ConnectivitySMILES")]
    smiles: String,
}

#[derive(Deserialize)]
struct CodRecord {
    file: String,
    #[serde(default)]
    commonname: Option<String>,
    #[serde(default)]
    chemname: Option<String>,
    #[serde(default)]
    mineral: Option<String>,
    #[serde(default)]
    formula: Option<String>,
    #[serde(default)]
    celltemp: Option<String>,
    #[serde(default)]
    diffrtemp: Option<String>,
    #[serde(default)]
    sg: Option<String>,
    #[serde(default, rename = "Robs")]
    robs: Option<String>,
    #[serde(default, rename = "Rall")]
    rall: Option<String>,
    #[serde(default)]
    doi: Option<String>,
    #[serde(default)]
    flags: Option<String>,
    #[serde(default)]
    svnrevision: Option<String>,
    #[serde(default)]
    duplicateof: Option<String>,
}

impl CodRecord {
    fn into_candidate(self, exact_formula: bool) -> CrystalCandidate {
        let formula = self.formula.unwrap_or_default();
        let name = self
            .commonname
            .or(self.chemname)
            .or(self.mineral)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| formula.clone());
        let temperature_k = parse_optional_number(self.celltemp.as_deref())
            .or_else(|| parse_optional_number(self.diffrtemp.as_deref()));
        let r_factor = parse_optional_number(self.robs.as_deref())
            .or_else(|| parse_optional_number(self.rall.as_deref()));
        let mut flags = self
            .flags
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        if self
            .duplicateof
            .as_deref()
            .is_some_and(|value| !value.is_empty())
        {
            flags.push("duplicate".to_string());
        }
        let revision = self.svnrevision;
        let mut warnings = Vec::new();
        if !exact_formula {
            warnings.push("substructure match; may be a salt, solvate, or derivative".to_string());
        }
        match temperature_k {
            Some(temperature) if !(273.0..=318.0).contains(&temperature) => {
                warnings.push(format!("measured at {temperature} K"));
            }
            None => warnings.push("measurement temperature is unavailable".to_string()),
            _ => {}
        }
        if r_factor.is_none() {
            warnings.push("R factor is unavailable".to_string());
        }
        if flags
            .iter()
            .any(|flag| flag.to_ascii_lowercase().contains("disorder"))
        {
            warnings.push("currently cannot be imported because disorder is present".to_string());
        }
        if revision.as_deref().is_none_or(str::is_empty) {
            warnings.push(
                "fixed COD revision is unavailable; this record cannot be imported".to_string(),
            );
        }
        CrystalCandidate {
            cod_id: self.file,
            revision,
            cid: None,
            smiles: None,
            name,
            formula,
            temperature_k,
            space_group: self.sg,
            r_factor,
            doi: self.doi.filter(|value| !value.trim().is_empty()),
            flags,
            exact_formula,
            warnings,
        }
    }
}

fn parse_optional_number(value: Option<&str>) -> Option<f64> {
    value?.trim().parse().ok()
}

#[cfg(test)]
#[path = "online_structures/tests.rs"]
mod tests;
