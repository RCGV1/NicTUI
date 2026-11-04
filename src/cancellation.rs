use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};

static REQUESTED: AtomicBool = AtomicBool::new(false);

/// Install once at a binary entrypoint, never implicitly for library callers.
pub fn install_handler() -> Result<()> {
    ctrlc::set_handler(|| REQUESTED.store(true, Ordering::SeqCst))
        .context("Failed to install cooperative interruption handler")
}

/// A sticky request; only the operation owner decides where cleanup is safe.
pub fn is_requested() -> bool {
    REQUESTED.load(Ordering::SeqCst)
}
