use std::{path::PathBuf, sync::mpsc};

use anyhow::Result;

use crate::io::online_structures::{
    CrystalCandidate, FetchedCod, StructureQuery, StructureSearchResult,
};

pub enum OnlineStructureJobOutcome {
    Search(Result<StructureSearchResult>),
    Fetch(Result<Box<FetchedCod>>),
}

pub struct RunningOnlineStructureJob {
    pub generation: u64,
    pub receiver: mpsc::Receiver<OnlineStructureJobOutcome>,
}

pub struct TrackedAgentOnlineStructureJob {
    pub id: u64,
    pub conversation: crate::frontend::agent::AssistantConversationId,
    pub running: RunningOnlineStructureJob,
}

pub fn spawn_online_structure_search(
    query: StructureQuery,
    generation: u64,
) -> RunningOnlineStructureJob {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(OnlineStructureJobOutcome::Search(
            crate::io::online_structures::search_structures(&query),
        ));
    });
    RunningOnlineStructureJob {
        generation,
        receiver,
    }
}

pub fn spawn_cod_fetch(
    query: String,
    candidate: CrystalCandidate,
    structures_dir: PathBuf,
    generation: u64,
) -> RunningOnlineStructureJob {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(OnlineStructureJobOutcome::Fetch(
            crate::io::online_structures::fetch_cod(&query, &candidate, &structures_dir)
                .map(Box::new),
        ));
    });
    RunningOnlineStructureJob {
        generation,
        receiver,
    }
}
