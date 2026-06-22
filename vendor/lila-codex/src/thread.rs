use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, RwLock};

use async_stream::try_stream;
use futures::{Stream, StreamExt};

use crate::codex_options::CodexOptions;
use crate::errors::{Error, Result};
use crate::events::{ThreadError, ThreadEvent, Usage};
use crate::exec::{CodexExec, CodexExecArgs};
use crate::items::ThreadItem;
use crate::output_schema_file::create_output_schema_file;
use crate::thread_options::ThreadOptions;
use crate::turn_options::TurnOptions;

/// Structured user input for multimodal turns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserInput {
    /// Plain text segment appended to the prompt.
    Text {
        /// Text content included in the prompt.
        text: String,
    },
    /// Local image path passed to Codex via `--image`.
    LocalImage {
        /// Path to a local image file.
        path: PathBuf,
    },
}

/// Input accepted by [`Thread::run`] / [`Thread::run_streamed`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    /// Single text prompt.
    Text(String),
    /// Ordered multimodal entries.
    Entries(Vec<UserInput>),
}

impl From<&str> for Input {
    fn from(value: &str) -> Self {
        Self::Text(value.to_string())
    }
}

impl From<String> for Input {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<Vec<UserInput>> for Input {
    fn from(value: Vec<UserInput>) -> Self {
        Self::Entries(value)
    }
}

/// Completed turn.
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    /// Completed items emitted during the turn.
    pub items: Vec<ThreadItem>,
    /// Final assistant response text from the latest `agent_message` item.
    pub final_response: String,
    /// Token usage when reported by the CLI.
    pub usage: Option<Usage>,
}

/// Alias for [`Turn`] to describe the result of [`Thread::run`].
pub type RunResult = Turn;

/// Stream of thread events.
pub type ThreadEventStream = Pin<Box<dyn Stream<Item = Result<ThreadEvent>> + Send>>;

/// Result of [`Thread::run_streamed`].
pub struct RunStreamedResult {
    /// Event stream for the current turn.
    pub events: ThreadEventStream,
}

/// Represents a thread of conversation with the agent.
#[derive(Debug, Clone)]
pub struct Thread {
    exec: CodexExec,
    options: CodexOptions,
    thread_options: ThreadOptions,
    id: Arc<RwLock<Option<String>>>,
}

impl Thread {
    pub(crate) fn new(
        exec: CodexExec,
        options: CodexOptions,
        thread_options: ThreadOptions,
        id: Option<String>,
    ) -> Self {
        Self {
            exec,
            options,
            thread_options,
            id: Arc::new(RwLock::new(id)),
        }
    }

    /// Returns the current thread id, if available.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use codex::Codex;
    ///
    /// let codex = Codex::new(None)?;
    /// let thread = codex.start_thread(None);
    /// let _id = thread.id();
    /// # Ok::<(), codex::Error>(())
    /// ```
    pub fn id(&self) -> Option<String> {
        self.id.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Provides input to the agent and streams events as they are produced.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use codex::{Codex, ThreadEvent};
    /// use futures::StreamExt;
    ///
    /// # async fn example() -> codex::Result<()> {
    /// let codex = Codex::new(None)?;
    /// let thread = codex.start_thread(None);
    /// let mut events = thread.run_streamed("Review this code", None).await?.events;
    ///
    /// while let Some(event) = events.next().await {
    ///     if let ThreadEvent::TurnCompleted { usage } = event? {
    ///         println!("{usage:?}");
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn run_streamed(
        &self,
        input: impl Into<Input>,
        turn_options: Option<TurnOptions>,
    ) -> Result<RunStreamedResult> {
        let input: Input = input.into();
        let turn_options = turn_options.unwrap_or_default();
        let schema_file = create_output_schema_file(turn_options.output_schema.as_ref())?;

        let (prompt, images) = normalize_input(input);
        let output_schema_file = schema_file
            .as_ref()
            .map(|file| file.path().to_string_lossy().into_owned());

        let exec_args = CodexExecArgs {
            input: prompt,
            base_url: self.options.base_url.clone(),
            api_key: self.options.api_key.clone(),
            thread_id: self.id(),
            images,
            model: self.thread_options.model.clone(),
            sandbox_mode: self.thread_options.sandbox_mode,
            working_directory: self.thread_options.working_directory.clone(),
            additional_directories: self
                .thread_options
                .additional_directories
                .clone()
                .unwrap_or_default(),
            skip_git_repo_check: self.thread_options.skip_git_repo_check.unwrap_or(false),
            output_schema_file,
            model_reasoning_effort: self.thread_options.model_reasoning_effort,
            network_access_enabled: self.thread_options.network_access_enabled,
            web_search_mode: self.thread_options.web_search_mode,
            web_search_enabled: self.thread_options.web_search_enabled,
            approval_policy: self.thread_options.approval_policy,
            cancellation_token: turn_options.cancellation_token.clone(),
        };

        let line_stream = self.exec.run(exec_args).await?;
        let id_handle = Arc::clone(&self.id);

        let events = try_stream! {
            let _schema_file = schema_file;
            let mut line_stream = line_stream;

            while let Some(line_result) = line_stream.next().await {
                let line = line_result?;
                let event: ThreadEvent = serde_json::from_str(&line)
                    .map_err(|e| Error::JsonParse(format!("{e}: {line}")))?;

                if let ThreadEvent::ThreadStarted { thread_id } = &event {
                    *id_handle.write().unwrap_or_else(|e| e.into_inner()) = Some(thread_id.clone());
                }

                yield event;
            }
        };

        Ok(RunStreamedResult {
            events: Box::pin(events),
        })
    }

    /// Provides input to the agent and returns the completed turn.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use codex::Codex;
    ///
    /// # async fn example() -> codex::Result<()> {
    /// let codex = Codex::new(None)?;
    /// let thread = codex.start_thread(None);
    /// let turn = thread.run("Summarize current repository state", None).await?;
    /// println!("{}", turn.final_response);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn run(
        &self,
        input: impl Into<Input>,
        turn_options: Option<TurnOptions>,
    ) -> Result<Turn> {
        let streamed = self.run_streamed(input, turn_options).await?;
        let mut events = streamed.events;

        let mut items = Vec::new();
        let mut final_response = String::new();
        let mut usage = None;
        let mut turn_failure: Option<ThreadError> = None;
        let mut stream_error: Option<String> = None;

        while let Some(event_result) = events.next().await {
            let event = event_result?;
            match event {
                ThreadEvent::ItemCompleted { item } => {
                    if let ThreadItem::AgentMessage(agent_message) = &item {
                        final_response = agent_message.text.clone();
                    }
                    items.push(item);
                }
                ThreadEvent::TurnCompleted { usage: turn_usage } => {
                    usage = Some(turn_usage);
                }
                ThreadEvent::TurnFailed { error } => {
                    turn_failure = Some(error);
                    break;
                }
                ThreadEvent::Error { message } => {
                    stream_error = Some(message);
                    break;
                }
                _ => {}
            }
        }

        if let Some(error) = turn_failure {
            return Err(Error::ThreadRun(error.message));
        }
        if let Some(message) = stream_error {
            return Err(Error::ThreadRun(message));
        }

        Ok(Turn {
            items,
            final_response,
            usage,
        })
    }
}

fn normalize_input(input: Input) -> (String, Vec<String>) {
    match input {
        Input::Text(text) => (text, Vec::new()),
        Input::Entries(entries) => {
            let mut prompt_parts = Vec::new();
            let mut images = Vec::new();

            for entry in entries {
                match entry {
                    UserInput::Text { text } => prompt_parts.push(text),
                    UserInput::LocalImage { path } => {
                        images.push(path.to_string_lossy().to_string())
                    }
                }
            }

            (prompt_parts.join("\n\n"), images)
        }
    }
}
