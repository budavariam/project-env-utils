pub mod backend;
pub mod backup;
pub mod cmd;
pub mod config;
pub mod env_file;
pub mod state;
pub mod tmux;

#[cfg(test)]
pub mod test_utils {
    use std::sync::Mutex;
    pub static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Set PENV_REPO_ROOT in tests. Safe because all tests hold ENV_LOCK.
    pub fn set_repo_root(path: &str) {
        // SAFETY: tests serialize env mutation via ENV_LOCK.
        unsafe { std::env::set_var("PENV_REPO_ROOT", path) }
    }

    /// Remove PENV_REPO_ROOT after a test.
    pub fn clear_repo_root() {
        // SAFETY: tests serialize env mutation via ENV_LOCK.
        unsafe { std::env::remove_var("PENV_REPO_ROOT") }
    }
}
