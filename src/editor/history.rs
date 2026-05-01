//! Snapshot based undo / redo stack.
//!
//! Every editor mutation that matters to the author runs through
//! `snapshot` before applying the change. The snapshot records a
//! full clone of the `LevelAst` at the moment immediately before
//! the mutation, so `undo` restores the document to that exact
//! state.
//!
//! # Memory budget
//!
//! `MAX_HISTORY` caps each stack at 32 entries. A typical
//! `LevelAst` is a few kilobytes; 32 entries is well under a
//! megabyte. Older entries are dropped from the bottom of the
//! stack when the limit is reached.
//!
//! # Interaction with saves
//!
//! History is not persisted to disk. A fresh editor session
//! starts with empty stacks. This is deliberate: restoring
//! undo history across a crash would bloat the session file
//! and present the author with unfamiliar "ghost" states that
//! do not correspond to anything they remember doing. Saving
//! the document also does not clear the history, so the
//! author can still undo past a save if they notice a mistake
//! right after persisting it.

use std::collections::VecDeque;

use crate::dsl::ast::LevelAst;

/// Maximum number of past or future states remembered. Tuned so
/// a typical session keeps at least a full editing arc (dozens
/// of actions) without ever blowing memory.
pub const MAX_HISTORY: usize = 32;

pub struct History {
    past: VecDeque<LevelAst>,
    future: VecDeque<LevelAst>,
}

impl History {
    pub fn new() -> Self {
        History {
            past: VecDeque::with_capacity(MAX_HISTORY),
            future: VecDeque::with_capacity(MAX_HISTORY),
        }
    }

    /// Record a pre-mutation snapshot. Call exactly once per
    /// user-visible action, right before applying the
    /// mutation to the live document. Doing so after the
    /// mutation is useless: undo would restore the post state.
    pub fn snapshot(&mut self, current: &LevelAst) {
        if self.past.len() >= MAX_HISTORY {
            self.past.pop_front();
        }
        self.past.push_back(current.clone());
        // A new action invalidates every previously redone
        // path. This matches standard editor semantics: once
        // the author branches, the old redo trail is gone.
        self.future.clear();
    }

    /// Pop a past state and hand it back for installation as
    /// the new current. The current state is pushed onto the
    /// future stack so `redo` can walk back. Returns `None`
    /// when there is nothing to undo.
    pub fn undo(&mut self, current: &LevelAst) -> Option<LevelAst> {
        let prev = self.past.pop_back()?;
        if self.future.len() >= MAX_HISTORY {
            self.future.pop_front();
        }
        self.future.push_back(current.clone());
        Some(prev)
    }

    /// Mirror of `undo`. Pops a future state and pushes the
    /// current one onto the past stack.
    pub fn redo(&mut self, current: &LevelAst) -> Option<LevelAst> {
        let next = self.future.pop_back()?;
        if self.past.len() >= MAX_HISTORY {
            self.past.pop_front();
        }
        self.past.push_back(current.clone());
        Some(next)
    }

    pub fn can_undo(&self) -> bool { !self.past.is_empty() }
    pub fn can_redo(&self) -> bool { !self.future.is_empty() }
}