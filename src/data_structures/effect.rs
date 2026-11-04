use super::{Action, InternalOp, JournalEntry, Modal, Remedy};

/// What a key press asks the app to do. Screens and modals return these instead of
/// reaching into app state.
pub enum Effect {
    /// The key was handled (e.g. moved a selection); nothing else to do.
    Nothing,
    /// Run an action: blocked ones only explain themselves, `confirm` ones ask first.
    Run { action: Action, via: String },
    /// Run without asking again (the user just confirmed).
    RunConfirmed { action: Action, via: String, parent: Option<i64> },
    /// Run a remedy from the error dialog, linked to the failed entry.
    Remedy { remedy: Remedy, parent: Option<i64> },
    Internal(InternalOp),
    Open(Box<Modal>),
    Close,
    /// Close the top modal, then do something.
    CloseThen(Box<Effect>),
    Message(String),
    SetLogFilter(String),
    SetJournalSearch(String),
    /// Undo one specific entry (`u` on the Journal screen).
    UndoEntry(Box<JournalEntry>),
}

impl Effect {
    pub fn run(action: Action, via: impl Into<String>) -> Effect {
        Effect::Run { action, via: via.into() }
    }

    pub fn close_then(e: Effect) -> Effect {
        Effect::CloseThen(Box::new(e))
    }

    pub fn open(m: Modal) -> Effect {
        Effect::Open(Box::new(m))
    }
}
