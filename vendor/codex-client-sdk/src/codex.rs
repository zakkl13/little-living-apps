use crate::codex_options::CodexOptions;
use crate::errors::Result;
use crate::exec::CodexExec;
use crate::thread::Thread;
use crate::thread_options::ThreadOptions;

/// Entry point for interacting with the Codex agent.
#[derive(Debug, Clone)]
pub struct Codex {
    exec: CodexExec,
    options: CodexOptions,
}

impl Codex {
    /// Creates a new Codex client.
    ///
    /// When `options` is `None`, default options are used and the SDK attempts
    /// to discover the `codex` executable automatically.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use codex::Codex;
    ///
    /// let _codex = Codex::new(None)?;
    /// # Ok::<(), codex::Error>(())
    /// ```
    pub fn new(options: Option<CodexOptions>) -> Result<Self> {
        let options = options.unwrap_or_default();
        let exec = CodexExec::new(
            options.codex_path_override.clone(),
            options.env.clone(),
            options.config.clone(),
        )?;
        Ok(Self { exec, options })
    }

    /// Starts a new thread.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use codex::Codex;
    ///
    /// let codex = Codex::new(None)?;
    /// let _thread = codex.start_thread(None);
    /// # Ok::<(), codex::Error>(())
    /// ```
    pub fn start_thread(&self, options: Option<ThreadOptions>) -> Thread {
        Thread::new(
            self.exec.clone(),
            self.options.clone(),
            options.unwrap_or_default(),
            None,
        )
    }

    /// Resumes an existing thread by id.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use codex::Codex;
    ///
    /// let codex = Codex::new(None)?;
    /// let _thread = codex.resume_thread("thread_123", None);
    /// # Ok::<(), codex::Error>(())
    /// ```
    pub fn resume_thread(&self, id: impl Into<String>, options: Option<ThreadOptions>) -> Thread {
        Thread::new(
            self.exec.clone(),
            self.options.clone(),
            options.unwrap_or_default(),
            Some(id.into()),
        )
    }
}
