//! herdr-flash: flash.nvim-style labeled jump, cursor and yank for Herdr pane copy.
//!
//! A [Herdr](https://herdr.dev) plugin popup that captures the focused pane's visible text, labels
//! matches with at most five one-key hints, lands a cursor on the picked target, and yanks a vim
//! selection through OSC 52.
//!
//! Herdr's own copy mode lives in the client. A Herdr that implements `pane.copy_mode_jump` (see
//! `docs/herdr-copy-mode-jump.patch`) lets a pick hand the cell over to that copy mode; every other
//! build — the request `RooseveltAdvisors/herdr-leap` calls does not exist upstream, see
//! herdrdev/herdr#2249 — gets the same picker with its own copy cursor instead.

pub mod app;
pub mod buffer;
pub mod clipboard;
pub mod config;
pub mod herdr_client;
pub mod hints;
pub mod jump;
pub mod matcher;
pub mod theme;
pub mod ui;
