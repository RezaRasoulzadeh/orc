use anyhow::{Context, Result};
use std::io::{self, Read, Write};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    CtrlC,
    Eof,
}

pub trait Terminal {
    fn read_key(&mut self) -> io::Result<Key>;
    fn render(&mut self, text: &str, cursor: usize) -> io::Result<()>;
    fn print(&mut self, text: &str) -> io::Result<()>;
}

pub struct StdioTerminal {
    input: io::Stdin,
    output: io::Stdout,
    bytes: Vec<u8>,
    _guard: TerminalStateGuard,
}

impl StdioTerminal {
    pub fn new() -> Result<Self> {
        Ok(Self {
            input: io::stdin(),
            output: io::stdout(),
            bytes: Vec::new(),
            _guard: TerminalStateGuard::new()?,
        })
    }
}

#[cfg(windows)]
impl StdioTerminal {
    fn read_windows_key(&mut self) -> io::Result<Key> {
        use windows_console::{
            EventData, GetStdHandle, InputRecord, ReadConsoleInputW, STD_INPUT_HANDLE,
        };
        loop {
            let mut record = InputRecord {
                kind: 0,
                data: EventData { raw: [0; 8] },
            };
            let mut read = 0;
            if unsafe {
                ReadConsoleInputW(GetStdHandle(STD_INPUT_HANDLE), &mut record, 1, &mut read)
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            if record.kind != 1 || read == 0 {
                continue;
            }
            let key = unsafe { record.data.key };
            if key.down == 0 {
                continue;
            }
            return Ok(match (key.vk, key.ch, key.control) {
                (0x25, _, _) => Key::Left,
                (0x27, _, _) => Key::Right,
                (0x26, _, _) => Key::Up,
                (0x28, _, _) => Key::Down,
                (0x2E, _, _) => Key::Delete,
                (0x08, _, _) => Key::Backspace,
                (_, 3, _) => Key::CtrlC,
                (_, 4, _) => Key::Eof,
                (_, 13, _) => Key::Enter,
                (_, c, _) => Key::Char(char::from_u32(c as u32).unwrap_or('\u{fffd}')),
            });
        }
    }
}

impl Terminal for StdioTerminal {
    fn read_key(&mut self) -> io::Result<Key> {
        #[cfg(windows)]
        {
            return self.read_windows_key();
        }
        loop {
            if let Some(key) = decode(&mut self.bytes)? {
                return Ok(key);
            }
            let mut byte = [0; 1];
            if self.input.read(&mut byte)? == 0 {
                return Ok(Key::Eof);
            }
            self.bytes.push(byte[0]);
        }
    }

    fn render(&mut self, text: &str, cursor: usize) -> io::Result<()> {
        let suffix = text[cursor..].chars().count();
        write!(self.output, "\r\x1b[2Korc> {text}")?;
        if suffix > 0 {
            write!(self.output, "\x1b[{suffix}D")?;
        }
        self.output.flush()
    }

    fn print(&mut self, text: &str) -> io::Result<()> {
        self.output.write_all(text.as_bytes())?;
        self.output.flush()
    }
}

pub trait TerminalStateBackend {
    type State;
    fn capture(&mut self) -> Result<Self::State>;
    fn setup(&mut self, state: &Self::State) -> Result<()>;
    fn restore(&mut self, state: &Self::State);
}

pub struct TerminalStateGuard<B: TerminalStateBackend = SystemTerminalStateBackend> {
    backend: B,
    state: B::State,
}

impl TerminalStateGuard<SystemTerminalStateBackend> {
    fn new() -> Result<Self> {
        Self::new_with_backend(SystemTerminalStateBackend)
    }
}

impl<B: TerminalStateBackend> TerminalStateGuard<B> {
    fn new_with_backend(mut backend: B) -> Result<Self> {
        let state = backend.capture()?;
        if let Err(error) = backend.setup(&state) {
            backend.restore(&state);
            return Err(error);
        }
        Ok(Self { backend, state })
    }
}

impl<B: TerminalStateBackend> Drop for TerminalStateGuard<B> {
    fn drop(&mut self) {
        self.backend.restore(&self.state);
    }
}

#[cfg(unix)]
pub struct SystemTerminalStateBackend;

#[cfg(unix)]
impl TerminalStateBackend for SystemTerminalStateBackend {
    type State = (i32, libc::termios);
    fn capture(&mut self) -> Result<Self::State> {
        let fd = libc::STDIN_FILENO;
        let mut original = std::mem::MaybeUninit::uninit();
        if unsafe { libc::tcgetattr(fd, original.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error()).context("read terminal state");
        }
        Ok((fd, unsafe { original.assume_init() }))
    }
    fn setup(&mut self, state: &Self::State) -> Result<()> {
        let mut raw = state.1;
        raw.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ISIG);
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;
        if unsafe { libc::tcsetattr(state.0, libc::TCSANOW, &raw) } != 0 {
            return Err(io::Error::last_os_error()).context("set raw terminal state");
        }
        Ok(())
    }
    fn restore(&mut self, state: &Self::State) {
        unsafe {
            libc::tcsetattr(state.0, libc::TCSANOW, &state.1);
        }
    }
}

#[cfg(windows)]
pub struct SystemTerminalStateBackend;

#[cfg(windows)]
impl TerminalStateBackend for SystemTerminalStateBackend {
    type State = (isize, isize, u32, u32);
    fn capture(&mut self) -> Result<Self::State> {
        use windows_console::*;
        let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
        let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
        let (mut input_mode, mut output_mode) = (0, 0);
        if input == 0
            || output == 0
            || unsafe { GetConsoleMode(input, &mut input_mode) } == 0
            || unsafe { GetConsoleMode(output, &mut output_mode) } == 0
        {
            return Err(io::Error::last_os_error()).context("read console mode");
        }
        Ok((input, output, input_mode, output_mode))
    }
    fn setup(&mut self, state: &Self::State) -> Result<()> {
        use windows_console::*;
        if unsafe { SetConsoleMode(state.0, state.2 & !ENABLE_PROCESSED_INPUT) } == 0 {
            return Err(io::Error::last_os_error()).context("set input console mode");
        }
        if unsafe { SetConsoleMode(state.1, state.3 | ENABLE_VIRTUAL_TERMINAL_PROCESSING) } == 0 {
            return Err(io::Error::last_os_error()).context("set output console mode");
        }
        Ok(())
    }
    fn restore(&mut self, state: &Self::State) {
        use windows_console::SetConsoleMode;
        unsafe {
            SetConsoleMode(state.0, state.2);
            SetConsoleMode(state.1, state.3);
        }
    }
}

#[cfg(windows)]
mod windows_console {
    pub type Handle = isize;
    pub const STD_INPUT_HANDLE: u32 = -10i32 as u32;
    pub const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    pub const ENABLE_PROCESSED_INPUT: u32 = 1;
    pub const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 4;
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct KeyEvent {
        pub down: i32,
        pub repeats: u16,
        pub vk: u16,
        pub scan: u16,
        pub ch: u16,
        pub control: u32,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub union EventData {
        pub key: KeyEvent,
        pub raw: [u16; 8],
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct InputRecord {
        pub kind: u16,
        pub data: EventData,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn GetStdHandle(which: u32) -> Handle;
        pub fn GetConsoleMode(handle: Handle, mode: *mut u32) -> i32;
        pub fn SetConsoleMode(handle: Handle, mode: u32) -> i32;
        pub fn ReadConsoleInputW(
            handle: Handle,
            record: *mut InputRecord,
            length: u32,
            read: *mut u32,
        ) -> i32;
    }
}

fn decode(bytes: &mut Vec<u8>) -> io::Result<Option<Key>> {
    if bytes.is_empty() {
        return Ok(None);
    }
    if bytes[0] == 3 {
        bytes.clear();
        return Ok(Some(Key::CtrlC));
    }
    if bytes[0] == 4 {
        bytes.remove(0);
        return Ok(Some(Key::Eof));
    }
    if bytes[0] == 0x1b {
        let sequences: &[(&[u8], Key)] = &[
            (b"\x1b[D", Key::Left),
            (b"\x1b[C", Key::Right),
            (b"\x1b[A", Key::Up),
            (b"\x1b[B", Key::Down),
            (b"\x1b[3~", Key::Delete),
        ];
        for (sequence, key) in sequences {
            if bytes.len() < sequence.len() && sequence.starts_with(bytes) {
                return Ok(None);
            }
            if bytes.starts_with(sequence) {
                bytes.drain(..sequence.len());
                return Ok(Some(key.clone()));
            }
        }
        if bytes.len() == 1 || (bytes.len() < 2 && bytes[1..].is_empty()) {
            return Ok(None);
        }
        bytes.remove(0);
        return Ok(Some(Key::Char('\x1b')));
    }
    if bytes[0] >= 128 {
        match std::str::from_utf8(bytes) {
            Ok(text) => {
                let c = text.chars().next().unwrap();
                bytes.drain(..c.len_utf8());
                return Ok(Some(Key::Char(c)));
            }
            Err(error) if error.error_len().is_none() => return Ok(None),
            Err(error) => {
                bytes.drain(..error.valid_up_to().max(1));
                return Ok(Some(Key::Char('\u{fffd}')));
            }
        }
    }
    let key = if bytes.starts_with(b"\x1b[D") {
        bytes.drain(..3);
        Key::Left
    } else if bytes.starts_with(b"\x1b[C") {
        bytes.drain(..3);
        Key::Right
    } else if bytes.starts_with(b"\x1b[A") {
        bytes.drain(..3);
        Key::Up
    } else if bytes.starts_with(b"\x1b[B") {
        bytes.drain(..3);
        Key::Down
    } else if bytes.starts_with(b"\x1b[3~") {
        bytes.drain(..4);
        Key::Delete
    } else {
        let b = bytes.remove(0);
        match b {
            b'\r' | b'\n' => Key::Enter,
            8 | 127 => Key::Backspace,
            _ => Key::Char(b as char),
        }
    };
    Ok(Some(key))
}

pub fn parse_arguments(line: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut started = false;
    for ch in line.chars() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (None, '\'' | '"') => {
                quote = Some(ch);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started {
                    out.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            (_, c) => {
                word.push(c);
                started = true;
            }
        }
    }
    if quote.is_some() {
        anyhow::bail!("unterminated quote")
    }
    if started {
        out.push(word);
    }
    Ok(out)
}

pub struct Editor<T> {
    terminal: T,
    history: Vec<String>,
    history_pos: Option<usize>,
    prompt: String,
}

impl<T: Terminal> Editor<T> {
    pub fn new(terminal: T) -> Self {
        Self {
            terminal,
            history: Vec::new(),
            history_pos: None,
            prompt: "orc> ".into(),
        }
    }
    pub fn run(&mut self) -> Result<()> {
        self.terminal.print(&self.prompt)?;
        let mut line = String::new();
        let mut cursor = 0;
        loop {
            match self.terminal.read_key()? {
                Key::Char(c) => {
                    line.insert(cursor, c);
                    cursor += c.len_utf8();
                    self.redraw(&line, cursor)?;
                }
                Key::Left => {
                    cursor = line[..cursor]
                        .char_indices()
                        .next_back()
                        .map_or(0, |(i, _)| i);
                    self.redraw(&line, cursor)?;
                }
                Key::Right => {
                    cursor = line[cursor..]
                        .chars()
                        .next()
                        .map_or(line.len(), |c| cursor + c.len_utf8());
                    self.redraw(&line, cursor)?;
                }
                Key::Backspace => {
                    if cursor > 0 {
                        let start = line[..cursor]
                            .char_indices()
                            .next_back()
                            .map_or(0, |(i, _)| i);
                        line.drain(start..cursor);
                        cursor = start;
                        self.redraw(&line, cursor)?;
                    }
                }
                Key::Delete => {
                    if cursor < line.len() {
                        let end = cursor + line[cursor..].chars().next().unwrap().len_utf8();
                        line.drain(cursor..end);
                        self.redraw(&line, cursor)?;
                    }
                }
                Key::Up => {
                    if !self.history.is_empty() {
                        let pos = self.history_pos.unwrap_or(self.history.len());
                        let next = pos.saturating_sub(1);
                        self.history_pos = Some(next);
                        line = self.history[next].clone();
                        cursor = line.len();
                        self.redraw(&line, cursor)?;
                    }
                }
                Key::Down => {
                    if let Some(pos) = self.history_pos {
                        if pos + 1 < self.history.len() {
                            self.history_pos = Some(pos + 1);
                            line = self.history[pos + 1].clone();
                        } else {
                            self.history_pos = None;
                            line.clear();
                        }
                        cursor = line.len();
                        self.redraw(&line, cursor)?;
                    }
                }
                Key::CtrlC => {
                    line.clear();
                    cursor = 0;
                    self.terminal.print("^C\r\n")?;
                    self.terminal.print(&self.prompt)?;
                }
                Key::Enter => {
                    let command = std::mem::take(&mut line);
                    cursor = 0;
                    self.history_pos = None;
                    if !command.is_empty() {
                        self.history.push(command.clone());
                    }
                    self.terminal.print("\r\n")?;
                    let args = parse_arguments(&command)?;
                    if matches!(args.first().map(String::as_str), Some("exit" | "quit")) {
                        return Ok(());
                    }
                    if matches!(args.first().map(String::as_str), Some("clear")) {
                        self.terminal.print("\x1b[2J\x1b[H")?;
                    } else if matches!(args.first().map(String::as_str), Some("help")) {
                        self.terminal.print("help history clear exit quit\r\n")?;
                    } else if matches!(args.first().map(String::as_str), Some("history")) {
                        for (index, entry) in self.history.iter().enumerate() {
                            self.terminal
                                .print(&format!("{}  {}\r\n", index + 1, entry))?;
                        }
                    }
                    self.terminal.print(&self.prompt)?;
                }
                Key::Eof => return Ok(()),
            }
        }
    }
    fn redraw(&mut self, line: &str, cursor: usize) -> io::Result<()> {
        self.terminal.render(line, cursor)
    }
}

pub fn run() -> Result<()> {
    Editor::new(StdioTerminal::new()?).run()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeTerminal {
        keys: Vec<Key>,
        output: String,
    }

    impl Terminal for FakeTerminal {
        fn read_key(&mut self) -> io::Result<Key> {
            Ok(self
                .keys
                .first()
                .cloned()
                .map_or(Key::Eof, |_| self.keys.remove(0)))
        }
        fn render(&mut self, text: &str, cursor: usize) -> io::Result<()> {
            self.output.push_str(&format!("render:{text}:{cursor};"));
            Ok(())
        }
        fn print(&mut self, text: &str) -> io::Result<()> {
            self.output.push_str(text);
            Ok(())
        }
    }

    #[test]
    fn parses_quoted_and_empty_arguments() {
        assert_eq!(
            parse_arguments("run 'two words' \"\"").unwrap(),
            ["run", "two words", ""]
        );
    }

    #[test]
    fn renders_prompt_immediately_and_exits() {
        let terminal = FakeTerminal {
            keys: vec![
                Key::Char('q'),
                Key::Char('u'),
                Key::Char('i'),
                Key::Char('t'),
                Key::Enter,
            ],
            output: String::new(),
        };
        let mut editor = Editor::new(terminal);
        editor.run().unwrap();
        assert!(editor.terminal.output.starts_with("orc> "));
    }

    #[test]
    fn inserts_at_cursor_and_handles_delete() {
        let terminal = FakeTerminal {
            keys: vec![
                Key::Char('a'),
                Key::Char('c'),
                Key::Left,
                Key::Char('b'),
                Key::Delete,
                Key::Enter,
                Key::Eof,
            ],
            output: String::new(),
        };
        let mut editor = Editor::new(terminal);
        editor.run().unwrap();
        assert!(editor.terminal.output.contains("render:abc:2;"));
        assert!(editor.terminal.output.contains("render:ab:2;"));
    }

    #[test]
    fn handles_backspace_and_history_navigation() {
        let terminal = FakeTerminal {
            keys: vec![
                Key::Char('a'),
                Key::Char('b'),
                Key::Backspace,
                Key::Enter,
                Key::Char('x'),
                Key::Enter,
                Key::Up,
                Key::Up,
                Key::Down,
                Key::Down,
                Key::Enter,
                Key::Char('q'),
                Key::Char('u'),
                Key::Char('i'),
                Key::Char('t'),
                Key::Enter,
            ],
            output: String::new(),
        };
        let mut editor = Editor::new(terminal);
        editor.run().unwrap();
        assert!(editor.terminal.output.contains("render:a:1;"));
        assert!(editor.terminal.output.contains("render:x:1;"));
        assert!(editor.terminal.output.contains("render::0;"));
    }

    #[test]
    fn ctrl_c_clears_line_and_starts_fresh_prompt() {
        let terminal = FakeTerminal {
            keys: vec![
                Key::Char('o'),
                Key::Char('n'),
                Key::Char('e'),
                Key::CtrlC,
                Key::Char('q'),
                Key::Char('u'),
                Key::Char('i'),
                Key::Char('t'),
                Key::Enter,
            ],
            output: String::new(),
        };
        let mut editor = Editor::new(terminal);
        editor.run().unwrap();
        assert!(editor.terminal.output.contains("^C\r\norc> "));
        assert!(editor.terminal.output.ends_with("render:quit:4;\r\n"));
    }

    #[test]
    fn built_in_commands_clear_and_quit() {
        let terminal = FakeTerminal {
            keys: vec![
                Key::Char('c'),
                Key::Char('l'),
                Key::Char('e'),
                Key::Char('a'),
                Key::Char('r'),
                Key::Enter,
                Key::Char('q'),
                Key::Char('u'),
                Key::Char('i'),
                Key::Char('t'),
                Key::Enter,
            ],
            output: String::new(),
        };
        let mut editor = Editor::new(terminal);
        editor.run().unwrap();
        assert!(editor.terminal.output.contains("\x1b[2J\x1b[H"));
        assert_eq!(editor.terminal.output.matches("orc> ").count(), 2);
    }

    #[test]
    fn literal_exit_terminates_the_session() {
        let terminal = FakeTerminal {
            keys: "exit\n"
                .chars()
                .map(|c| if c == '\n' { Key::Enter } else { Key::Char(c) })
                .collect(),
            output: String::new(),
        };
        let mut editor = Editor::new(terminal);
        editor.run().unwrap();
        assert_eq!(editor.terminal.output.matches("orc> ").count(), 1);
    }

    struct FakeStateBackend {
        state: std::rc::Rc<std::cell::Cell<u32>>,
        original: u32,
        replacement: u32,
        fail_after_setup: bool,
    }

    impl TerminalStateBackend for FakeStateBackend {
        type State = u32;
        fn capture(&mut self) -> Result<Self::State> {
            Ok(self.original)
        }
        fn setup(&mut self, state: &Self::State) -> Result<()> {
            self.state.set(self.replacement);
            if self.fail_after_setup {
                anyhow::bail!("test setup failure")
            }
            assert_eq!(*state, self.original);
            Ok(())
        }
        fn restore(&mut self, state: &Self::State) {
            self.state.set(*state);
        }
    }

    #[test]
    fn terminal_state_guard_restores_exact_state_on_success() {
        let state = std::rc::Rc::new(std::cell::Cell::new(0b1011));
        let guard = TerminalStateGuard::new_with_backend(FakeStateBackend {
            state: state.clone(),
            original: 0b1011,
            replacement: 0b0100,
            fail_after_setup: false,
        })
        .unwrap();
        assert_eq!(state.get(), 0b0100);
        drop(guard);
        assert_eq!(state.get(), 0b1011);
    }

    #[test]
    fn terminal_state_guard_restores_after_setup_failure() {
        let state = std::rc::Rc::new(std::cell::Cell::new(0b1101));
        let result = TerminalStateGuard::new_with_backend(FakeStateBackend {
            state: state.clone(),
            original: 0b1101,
            replacement: 0b0010,
            fail_after_setup: true,
        });
        assert!(result.is_err());
        assert_eq!(state.get(), 0b1101);
    }

    #[test]
    fn eof_terminates_and_redraw_tracks_cursor() {
        let terminal = FakeTerminal {
            keys: vec![
                Key::Char('a'),
                Key::Char('c'),
                Key::Left,
                Key::Right,
                Key::Eof,
            ],
            output: String::new(),
        };
        let mut editor = Editor::new(terminal);
        editor.run().unwrap();
        assert!(editor.terminal.output.contains("render:ac:1;"));
        assert!(editor.terminal.output.contains("render:ac:2;"));
    }

    #[test]
    fn decodes_complete_escape_sequences_without_leaking_bytes() {
        for (input, expected) in [
            (b"\x1b[D".as_slice(), Key::Left),
            (b"\x1b[C".as_slice(), Key::Right),
            (b"\x1b[A".as_slice(), Key::Up),
            (b"\x1b[B".as_slice(), Key::Down),
            (b"\x1b[3~".as_slice(), Key::Delete),
        ] {
            let mut bytes = input[..1].to_vec();
            assert_eq!(decode(&mut bytes).unwrap(), None);
            bytes.extend_from_slice(&input[1..]);
            assert_eq!(decode(&mut bytes).unwrap(), Some(expected));
            assert!(bytes.is_empty());
        }
    }

    #[test]
    fn decodes_ctrl_c_and_ctrl_d() {
        let mut ctrl_c = vec![3];
        assert_eq!(decode(&mut ctrl_c).unwrap(), Some(Key::CtrlC));
        let mut eof = vec![4];
        assert_eq!(decode(&mut eof).unwrap(), Some(Key::Eof));
    }
}
