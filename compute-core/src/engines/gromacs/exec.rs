//! Thin wrapper for running a single `gmx` sub-tool through the shared
//! subprocess runner in [`super::runner`].

use std::{
    path::Path,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use anyhow::Result;

use crate::engines::gromacs::runner::{GromacsProgress, SubprocessOutcome, run_subprocess};
use crate::launch::Compute;

pub(crate) fn run_gmx<F>(
    compute: &Compute,
    working_dir: &Path,
    args: Vec<String>,
    stdin: Option<Vec<u8>>,
    timeout: Duration,
    cancel: Arc<AtomicBool>,
    report: &mut F,
) -> Result<SubprocessOutcome>
where
    F: FnMut(GromacsProgress),
{
    let mut config =
        compute
            .launch
            .to_process_config(working_dir.to_path_buf(), args, Some(timeout));
    if let Some(stdin) = stdin {
        config = config.stdin_bytes(stdin);
    }
    run_subprocess(config, cancel, report)
}
