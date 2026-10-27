//! A terminal for tests, which opens no window.
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

/// Stands for the terminal of the OS. It opens no window: it records the
/// executable it was given and answers what `fails` says.
#[derive(Clone, Default)]
pub(crate) struct Terminal {
    pub opened: Arc<Mutex<Vec<PathBuf>>>,
    pub fails: Arc<AtomicBool>,
}

impl Terminal {
    pub fn open(&self, executable: &Path) -> bool {
        self.opened.lock().unwrap().push(executable.into());
        !self.fails.load(Ordering::SeqCst)
    }
}
