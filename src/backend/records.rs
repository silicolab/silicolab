use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};

pub mod artifacts;
pub mod qm;
#[cfg(test)]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Evidence,
    Memory,
    Derived,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Scope {
    pub task: Option<u64>,
    pub session: Option<u64>,
    pub run_uuid: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "authority", rename_all = "snake_case")]
pub enum Source {
    Program {
        job: String,
    },
    UserApproval {
        session: u64,
        call: String,
    },
    Model {
        generator: String,
        model: String,
        bases: Vec<Basis>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Basis {
    pub id: String,
    pub revision: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Content {
    Qm(Box<qm::QmFacts>),
    QmInput(Box<crate::engines::qm::QmJob>),
    Constraint {
        text: String,
    },
    Explanation {
        text: String,
    },
    #[cfg(test)]
    TestMeasurement {
        value: u32,
    },
}

impl Content {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Qm(_) => "qm.facts",
            Self::QmInput(_) => "qm.input",
            Self::Constraint { .. } => "task.constraint",
            Self::Explanation { .. } => "derived.explanation",
            #[cfg(test)]
            Self::TestMeasurement { .. } => "test.measurement",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub id: String,
    pub category: Category,
    pub storage_version: u32,
    pub content_version: u32,
    pub revision: u32,
    pub source: Source,
    pub scope: Scope,
    pub created_at_ms: u64,
    pub supersedes: Option<String>,
    pub invalidated: Option<String>,
    pub brief: String,
    pub input_entries: Vec<u64>,
    pub result_entries: Vec<u64>,
    pub artifacts: Vec<artifacts::Artifact>,
    pub content: Content,
}

impl Record {
    pub fn job(&self) -> Option<&str> {
        match &self.source {
            Source::Program { job } => Some(job),
            _ => None,
        }
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.storage_version == 1,
            "unsupported record storage version {}",
            self.storage_version
        );
        ensure!(
            self.content_version == 1,
            "unsupported content version {}",
            self.content_version
        );
        ensure!(
            !self.id.is_empty() && self.id.len() <= 200 && self.revision > 0,
            "invalid record identity"
        );
        ensure!(
            self.brief.chars().count() <= 240,
            "record brief exceeds budget"
        );
        let valid = matches!(
            (&self.category, &self.source, &self.content),
            (
                Category::Evidence,
                Source::Program { .. },
                Content::Qm(_) | Content::QmInput(_)
            ) | (
                Category::Memory,
                Source::UserApproval { .. },
                Content::Constraint { .. }
            ) | (
                Category::Derived,
                Source::Model { .. },
                Content::Explanation { .. }
            )
        );
        #[cfg(test)]
        let valid = valid
            || matches!(
                (&self.category, &self.source, &self.content),
                (
                    Category::Evidence,
                    Source::Program { .. },
                    Content::TestMeasurement { .. }
                )
            );
        ensure!(valid, "content and authority mismatch");
        if let Content::Constraint { text } | Content::Explanation { text } = &self.content {
            ensure!(
                !text.trim().is_empty() && text.chars().count() <= 1200,
                "text must contain 1..1200 characters"
            );
        }
        if let Source::UserApproval { session, call } = &self.source {
            ensure!(
                !call.is_empty() && self.scope.session == Some(*session),
                "constraint requires its approving session"
            );
        }
        if let Source::Model {
            generator,
            model,
            bases,
        } = &self.source
        {
            ensure!(
                !generator.is_empty() && !model.is_empty() && !bases.is_empty(),
                "derived records require generator, model and versioned bases"
            );
        }
        for artifact in &self.artifacts {
            artifact.validate()?;
            ensure!(
                self.scope.task == Some(artifact.task),
                "artifact task differs from record scope"
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub struct RecordStore {
    records: BTreeMap<String, Record>,
    pub unavailable: BTreeMap<String, (String, String)>,
    dirty: bool,
}

impl RecordStore {
    pub fn all(&self) -> impl Iterator<Item = &Record> {
        self.records.values()
    }
    pub fn get(&self, id: &str) -> Option<&Record> {
        self.records.get(id)
    }
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }
    pub fn mark_saved(&mut self) {
        self.dirty = false;
    }
    pub fn insert(&mut self, record: Record) -> Result<bool> {
        record.validate()?;
        if let Some(existing) = self.get(&record.id) {
            ensure!(
                serde_json::to_string(existing)? == serde_json::to_string(&record)?,
                "record identity already exists with different content"
            );
            return Ok(false);
        }
        ensure!(
            !self.unavailable.contains_key(&record.id),
            "record is unavailable; refusing to overwrite preserved data"
        );
        if let Some(id) = &record.supersedes {
            let previous = self
                .get(id)
                .ok_or_else(|| anyhow::anyhow!("replacement target not found: {id}"))?;
            ensure!(
                previous.category == record.category && previous.scope == record.scope,
                "replacement must retain category and scope"
            );
            ensure!(self.active(previous), "replacement target is not active");
        }
        ensure!(!self.stale(&record), "basis missing, stale or cyclic");
        self.records.insert(record.id.clone(), record);
        self.dirty = true;
        Ok(true)
    }
    pub fn restore(&mut self, id: String, json: String) {
        let parsed = serde_json::from_str::<Record>(&json)
            .map_err(anyhow::Error::from)
            .and_then(|r| {
                r.validate()?;
                ensure!(r.id == id, "record id mismatch");
                Ok(r)
            });
        match parsed {
            Ok(record) => {
                self.records.insert(id, record);
            }
            Err(error) => {
                self.unavailable.insert(id, (json, error.to_string()));
            }
        }
    }
    pub fn active(&self, record: &Record) -> bool {
        record.invalidated.is_none()
            && !self
                .all()
                .any(|r| r.supersedes.as_deref() == Some(&record.id))
    }
    pub fn stale(&self, record: &Record) -> bool {
        let mut pending = vec![(record, false)];
        let mut visiting = BTreeSet::new();
        let mut checked = BTreeSet::new();
        // Restored records can contain arbitrarily deep or cyclic dependencies.
        while let Some((current, leaving)) = pending.pop() {
            if leaving {
                visiting.remove(current.id.as_str());
                checked.insert(current.id.as_str());
                continue;
            }
            if checked.contains(current.id.as_str()) {
                continue;
            }
            if !visiting.insert(current.id.as_str()) {
                return true;
            }
            pending.push((current, true));
            if let Source::Model { bases, .. } = &current.source {
                for basis in bases {
                    let Some(base) = self.get(&basis.id) else {
                        return true;
                    };
                    if base.revision != basis.revision
                        || !self.active(base)
                        || record.supersedes.as_deref() == Some(base.id.as_str())
                    {
                        return true;
                    }
                    pending.push((base, false));
                }
            }
        }
        false
    }
    pub fn context(
        &self,
        task: Option<u64>,
        run_uuid: Option<&str>,
        session: u64,
        budget: usize,
    ) -> String {
        let mut out = String::new();
        let mut omitted = 0;
        for r in self.all().filter(|r| {
            r.category == Category::Memory
                && self.active(r)
                && r.scope.session == Some(session)
                && r.scope
                    .task
                    .is_none_or(|t| Some(t) == task && r.scope.run_uuid.as_deref() == run_uuid)
        }) {
            let line = format!(
                "User-confirmed constraint [{}]: {}\n",
                r.id,
                match &r.content {
                    Content::Constraint { text } => text.as_str(),
                    _ => continue,
                }
            );
            if out.chars().count() + line.chars().count() <= budget.saturating_sub(100) {
                out.push_str(&line);
            } else {
                omitted += 1;
            }
        }
        for r in self.all().filter(|r| {
            r.category == Category::Evidence
                && task.is_some()
                && r.scope.task == task
                && r.scope.run_uuid.as_deref() == run_uuid
                && self.active(r)
        }) {
            let line = format!("Relevant evidence [{}]: {}\n", r.id, r.brief);
            if out.chars().count() + line.chars().count() <= budget.saturating_sub(100) {
                out.push_str(&line);
            } else {
                omitted += 1;
            }
        }
        if omitted > 0 {
            out.push_str(&format!(
                "{omitted} applicable records omitted by budget; retrieve with inspect.\n"
            ));
        }
        out.chars().take(budget).collect()
    }
    pub fn register_artifacts(
        &mut self,
        id: &str,
        artifacts: Vec<artifacts::Artifact>,
    ) -> Result<()> {
        for artifact in &artifacts {
            artifact.validate()?;
        }
        let record = self
            .records
            .get_mut(id)
            .ok_or_else(|| anyhow::anyhow!("record not found"))?;
        record.artifacts = artifacts;
        self.dirty = true;
        Ok(())
    }
    pub fn bind_results(&mut self, job: &str, entries: &[u64]) {
        for r in self
            .records
            .values_mut()
            .filter(|r| r.job() == Some(job) && matches!(r.content, Content::Qm(_)))
        {
            if r.result_entries.is_empty() && !entries.is_empty() {
                r.result_entries = entries.to_vec();
                self.dirty = true;
            }
        }
    }
    pub fn require(&self, id: &str) -> Result<&Record> {
        if let Some((_, reason)) = self.unavailable.get(id) {
            bail!("record {id} unavailable: {reason}");
        }
        self.get(id)
            .ok_or_else(|| anyhow::anyhow!("record not found: {id}"))
    }
}
