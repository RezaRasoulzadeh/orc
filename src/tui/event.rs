use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyEvent};

pub fn next_key() -> Result<Option<KeyEvent>> {
    if !event::poll(Duration::from_millis(250))? {
        return Ok(None);
    }
    Ok(match event::read()? {
        Event::Key(key) => Some(key),
        _ => None,
    })
}
