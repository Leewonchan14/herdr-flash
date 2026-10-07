//! herdr-flash: flash.nvim-style labeled jump, cursor and yank for Herdr pane copy.
//!
//! A [Herdr](https://herdr.dev) plugin popup that captures the focused pane's visible text, labels
//! matches with at most five one-key hints, lands a cursor on the picked target, and yanks a vim
//! selection through OSC 52.
//!
//! Herdr's own copy mode lives in the client, and no released Herdr exposes an API that places its
//! cursor (the request that `RooseveltAdvisors/herdr-leap` calls, `pane.copy_mode_jump`, does not
//! exist — see herdrdev/herdr#2249). herdr-flash therefore brings the copy cursor into its own
//! popup instead of pretending to move the client's.

pub mod app;
pub mod buffer;
pub mod clipboard;
pub mod config;
pub mod herdr_client;
pub mod hints;
pub mod matcher;
pub mod theme;
pub mod ui;
