use crate::runtime::{Runtime, RuntimeEvent, RuntimeRequest, render_event};
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
    fn read_key_timeout(&mut self, _timeout: std::time::Duration) -> io::Result<Option<Key>> {
        self.read_key().map(Some)
    }
    fn render(&mut self, text: &str, cursor: usize) -> io::Result<()>;
    fn print(&mut self, text: &str) -> io::Result<()>;
}

pub trait RuntimePort {
    fn submit(
        &mut self,
        request: RuntimeRequest,
    ) -> (crate::runtime::OperationId, crate::runtime::Cancellation);
    fn recv(&self) -> Result<RuntimeEvent, std::sync::mpsc::RecvError>;
    fn try_recv(&self) -> Result<RuntimeEvent, std::sync::mpsc::TryRecvError>;
    fn cancel(&self, cancellation: &crate::runtime::Cancellation);
}

impl RuntimePort for Runtime {
    fn submit(
        &mut self,
        request: RuntimeRequest,
    ) -> (crate::runtime::OperationId, crate::runtime::Cancellation) {
        Runtime::submit(self, request)
    }
    fn recv(&self) -> Result<RuntimeEvent, std::sync::mpsc::RecvError> {
        Runtime::recv(self)
    }
    fn try_recv(&self) -> Result<RuntimeEvent, std::sync::mpsc::TryRecvError> {
        Runtime::try_recv(self)
    }
    fn cancel(&self, cancellation: &crate::runtime::Cancellation) {
        Runtime::cancel(self, cancellation)
    }
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

    fn read_key_timeout(&mut self, timeout: std::time::Duration) -> io::Result<Option<Key>> {
        #[cfg(unix)]
        {
            let mut set = unsafe { std::mem::zeroed::<libc::fd_set>() };
            unsafe {
                libc::FD_ZERO(&mut set);
                libc::FD_SET(libc::STDIN_FILENO, &mut set);
            }
            let mut tv = libc::timeval {
                tv_sec: timeout.as_secs() as _,
                tv_usec: timeout.subsec_micros() as _,
            };
            let ready = unsafe {
                libc::select(
                    libc::STDIN_FILENO + 1,
                    &mut set,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut tv,
                )
            };
            if ready == 0 {
                return Ok(None);
            }
            if ready < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        self.read_key().map(Some)
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

pub struct Editor<T, R = Runtime> {
    terminal: T,
    history: Vec<String>,
    history_pos: Option<usize>,
    prompt: String,
    runtime: Option<R>,
    confirmation: Option<String>,
    selection: Option<(String, Vec<String>, usize)>,
}

impl<T: Terminal> Editor<T, Runtime> {
    pub fn new(terminal: T) -> Self {
        Self {
            terminal,
            history: Vec::new(),
            history_pos: None,
            prompt: "orc> ".into(),
            runtime: None,
            confirmation: None,
            selection: None,
        }
    }
}

impl<T: Terminal, R: RuntimePort> Editor<T, R> {
    pub fn new_with_runtime(terminal: T, runtime: R) -> Self {
        Self {
            terminal,
            history: Vec::new(),
            history_pos: None,
            prompt: "orc> ".into(),
            runtime: Some(runtime),
            confirmation: None,
            selection: None,
        }
    }
    pub fn with_runtime(mut self, runtime: R) -> Self {
        self.runtime = Some(runtime);
        self
    }
    pub fn run(&mut self) -> Result<()> {
        if self.runtime.is_some() {
            self.refresh_context()?;
        }
        self.terminal.print(&self.prompt)?;
        let mut line = String::new();
        let mut cursor = 0;
        let mut active: Option<(crate::runtime::OperationId, crate::runtime::Cancellation)> = None;
        loop {
            let mut refresh = false;
            if let Some(runtime) = self.runtime.as_ref() {
                let mut events = Vec::new();
                while let Ok(event) = runtime.try_recv() {
                    events.push(event);
                }
                for event in events {
                    let finished = matches!(&event, RuntimeEvent::Completed(id, _) | RuntimeEvent::Failed(id, _) | RuntimeEvent::Cancelled(id) if active.as_ref().is_some_and(|(active_id, _)| *active_id == *id));
                    self.terminal.print(&render_event(&event))?;
                    if let RuntimeEvent::Completed(_, value) = &event
                        && let crate::runtime::RuntimeValue::AgentCandidates { task_id, agents } =
                            value.as_ref()
                    {
                        if agents.is_empty() {
                            self.terminal.print("error: no eligible agent found\r\n")?;
                        } else if agents.len() == 1 {
                            let (next, cancellation) = self
                                .runtime
                                .as_mut()
                                .context("runtime unavailable")?
                                .submit(RuntimeRequest::Dispatch {
                                    task_id: task_id.clone(),
                                    agent_id: Some(agents[0].clone()),
                                });
                            active = Some((next, cancellation));
                        } else {
                            self.selection = Some((task_id.clone(), agents.clone(), 0));
                            self.render_selection()?;
                        }
                    }
                    let candidate_request = matches!(
                        &event,
                        RuntimeEvent::Completed(_, value)
                            if matches!(value.as_ref(), crate::runtime::RuntimeValue::AgentCandidates { agents, .. } if !agents.is_empty())
                    );
                    if finished && !candidate_request {
                        active = None;
                        refresh = true;
                    }
                }
            }
            if refresh {
                self.refresh_context()?;
                self.terminal.print(&self.prompt)?;
            }
            let key = if active.is_some() {
                self.terminal
                    .read_key_timeout(std::time::Duration::from_millis(20))?
            } else {
                Some(self.terminal.read_key()?)
            };
            let Some(key) = key else { continue };
            if let Some((task_id, agents, selected)) = self.selection.clone() {
                match key {
                    Key::Up => self.selection = Some((task_id, agents, selected.saturating_sub(1))),
                    Key::Down => {
                        let next = (selected + 1).min(agents.len() - 1);
                        self.selection = Some((task_id, agents, next))
                    }
                    Key::Enter => {
                        let (id, cancellation) = self
                            .runtime
                            .as_mut()
                            .context("runtime unavailable")?
                            .submit(RuntimeRequest::Dispatch {
                                task_id,
                                agent_id: Some(agents[selected].clone()),
                            });
                        active = Some((id, cancellation));
                        self.selection = None;
                        self.terminal
                            .print("agent selected; dispatch submitted\r\n")?;
                    }
                    Key::CtrlC => {
                        self.selection = None;
                        self.terminal.print("selection cancelled\r\n")?;
                    }
                    _ => {}
                }
                continue;
            }
            if let Some(task_id) = self.confirmation.clone() {
                match key {
                    Key::Char('y') | Key::Char('Y') => {
                        self.confirmation = None;
                        if let Some(runtime) = self.runtime.as_mut() {
                            let (id, cancellation) =
                                runtime.submit(RuntimeRequest::CancelTask(task_id));
                            active = Some((id, cancellation));
                            self.terminal
                                .print("confirmed; cancellation submitted\r\n")?;
                        }
                        self.terminal.print(&self.prompt)?;
                    }
                    Key::Char('n') | Key::Char('N') | Key::CtrlC => {
                        self.confirmation = None;
                        self.terminal
                            .print("cancelled; no task state changed\r\n")?;
                        self.terminal.print(&self.prompt)?;
                    }
                    _ => {}
                }
                continue;
            }
            match key {
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
                    if let (Some((_, cancellation)), Some(runtime)) =
                        (active.as_ref(), self.runtime.as_ref())
                    {
                        runtime.cancel(cancellation);
                        self.terminal.print("^C\r\ncancelling...\r\n")?;
                        continue;
                    }
                    line.clear();
                    cursor = 0;
                    self.terminal.print("^C\r\n")?;
                    self.terminal.print(&self.prompt)?;
                }
                Key::Enter => {
                    if active.is_some() {
                        self.terminal
                            .print("operation active; press Ctrl-C to cancel\r\n")?;
                        continue;
                    }
                    let command = std::mem::take(&mut line);
                    cursor = 0;
                    self.history_pos = None;
                    if !command.is_empty() {
                        self.history.push(command.clone());
                    }
                    self.terminal.print("\r\n")?;
                    let args = match parse_arguments(&command) {
                        Ok(args) => args,
                        Err(error) => {
                            self.terminal.print(&format!("error: {error}\r\n"))?;
                            self.terminal.print(&self.prompt)?;
                            continue;
                        }
                    };
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
                    } else if let Some(runtime) = self.runtime.as_mut() {
                        if matches!(args.as_slice(), [command, _] if command == "cancel") {
                            self.confirmation = args.get(1).cloned();
                            self.terminal.print("confirm task cancellation? [y/N] ")?;
                        } else if args.len() == 2 && args[0] == "dispatch" {
                            let (id, cancellation) =
                                runtime.submit(RuntimeRequest::DispatchCandidates(args[1].clone()));
                            active = Some((id, cancellation));
                            self.terminal
                                .print(&format!("operation {} submitted\r\n", id.0))?;
                        } else if let Some(request) = runtime_request(&args) {
                            let (id, cancellation) = runtime.submit(request);
                            active = Some((id, cancellation));
                            self.terminal
                                .print(&format!("operation {} submitted\r\n", id.0))?;
                        } else if !args.is_empty() {
                            self.terminal
                                .print("error: unknown interactive command\r\n")?;
                        }
                    }
                    self.terminal.print(&self.prompt)?;
                }
                Key::Eof => return Ok(()),
            }
        }
    }
    fn refresh_context(&mut self) -> Result<()> {
        let runtime = self.runtime.as_mut().context("runtime unavailable")?;
        let (id, _) = runtime.submit(RuntimeRequest::ProjectStatus);
        loop {
            let event = runtime.recv().context("runtime event stream closed")?;
            if matches!(&event, RuntimeEvent::Context(event_id, _) if *event_id == id) {
                self.terminal.print(&render_event(&event))?;
            } else if matches!(&event, RuntimeEvent::Completed(event_id, _) | RuntimeEvent::Failed(event_id, _) | RuntimeEvent::Cancelled(event_id) if *event_id == id)
            {
                self.terminal.print(&render_event(&event))?;
                return Ok(());
            }
        }
    }
    fn redraw(&mut self, line: &str, cursor: usize) -> io::Result<()> {
        self.terminal.render(line, cursor)
    }
    fn render_selection(&mut self) -> Result<()> {
        if let Some((_, agents, selected)) = &self.selection {
            self.terminal.print("select dispatch agent:\r\n")?;
            for (index, agent) in agents.iter().enumerate() {
                self.terminal.print(&format!(
                    "{}{}\r\n",
                    if index == *selected { "> " } else { "  " },
                    agent
                ))?;
            }
        }
        Ok(())
    }
}

pub fn run() -> Result<()> {
    let runtime = Runtime::open(".orc/orc.db", ".")?;
    Editor::new(StdioTerminal::new()?)
        .with_runtime(runtime)
        .run()
}

fn runtime_request(args: &[String]) -> Option<RuntimeRequest> {
    match args {
        [command] if command == "status" || command == "project/status" => {
            Some(RuntimeRequest::ProjectStatus)
        }
        [command] if command == "tasks" => Some(RuntimeRequest::Tasks),
        [command, id] if command == "task" || command == "task/show" => {
            Some(RuntimeRequest::TaskShow(id.clone()))
        }
        [command] if command == "queue" => Some(RuntimeRequest::Queue),
        [command] if command == "runs" => Some(RuntimeRequest::Runs(20)),
        [command] if command == "agents" => Some(RuntimeRequest::Agents),
        [command, id] if command == "cancel" => Some(RuntimeRequest::CancelTask(id.clone())),
        [command, id] if command == "dispatch" => Some(RuntimeRequest::Dispatch {
            task_id: id.clone(),
            agent_id: None,
        }),
        [command, id, agent] if command == "dispatch" => Some(RuntimeRequest::Dispatch {
            task_id: id.clone(),
            agent_id: Some(agent.clone()),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{Cancellation, OperationId, RuntimeValue, SessionContext};

    struct FakeTerminal {
        keys: Vec<Key>,
        timed_keys: Vec<Option<Key>>,
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
        fn read_key_timeout(&mut self, _timeout: std::time::Duration) -> io::Result<Option<Key>> {
            Ok(if self.timed_keys.is_empty() {
                None
            } else {
                self.timed_keys.remove(0)
            })
        }
        fn print(&mut self, text: &str) -> io::Result<()> {
            self.output.push_str(text);
            Ok(())
        }
    }

    fn runtime_fixture() -> (tempfile::TempDir, Runtime) {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join(".orc")).unwrap();
        std::fs::write(directory.path().join(".orc/engineering.md"), "# Contract").unwrap();
        let runtime = Runtime::open(directory.path().join("orc.db"), directory.path()).unwrap();
        (directory, runtime)
    }

    fn command_keys(command: &str) -> Vec<Key> {
        command
            .chars()
            .map(Key::Char)
            .chain([Key::Enter, Key::Eof])
            .collect()
    }

    #[test]
    fn runtime_session_renders_startup_context_and_recovers_from_error() {
        let (_directory, runtime) = runtime_fixture();
        let terminal = FakeTerminal {
            keys: command_keys("dispatch missing"),
            timed_keys: Vec::new(),
            output: String::new(),
        };
        let mut editor = Editor::new(terminal).with_runtime(runtime);
        editor.run().unwrap();
        assert!(editor.terminal.output.contains("project: none\r\n"));
        assert!(editor.terminal.output.contains("error:"));
        assert!(editor.terminal.output.contains("orc> "));
    }

    #[test]
    fn runtime_session_completes_without_an_additional_keypress() {
        let (_directory, runtime) = runtime_fixture();
        let terminal = FakeTerminal {
            keys: command_keys("status"),
            timed_keys: Vec::new(),
            output: String::new(),
        };
        let mut editor = Editor::new(terminal).with_runtime(runtime);
        editor.run().unwrap();
        assert!(editor.terminal.output.contains("success:"));
        assert!(editor.terminal.output.matches("orc> ").count() >= 2);
    }

    #[test]
    fn malformed_command_recovers_to_an_active_prompt() {
        let terminal = FakeTerminal {
            keys: command_keys("status '")
                .into_iter()
                .chain(command_keys("quit"))
                .collect(),
            timed_keys: Vec::new(),
            output: String::new(),
        };
        let mut editor = Editor::new(terminal);
        editor.run().unwrap();
        assert!(
            editor
                .terminal
                .output
                .contains("error: unterminated quote\r\n")
        );
        assert!(editor.terminal.output.matches("orc> ").count() >= 2);
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
            timed_keys: Vec::new(),
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
            timed_keys: Vec::new(),
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
            timed_keys: Vec::new(),
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
            timed_keys: Vec::new(),
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
            timed_keys: Vec::new(),
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
            timed_keys: Vec::new(),
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
            timed_keys: Vec::new(),
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

    use std::cell::RefCell;
    use std::collections::{HashMap, VecDeque};
    use std::rc::Rc;

    enum FakeEvent {
        Lifecycle(&'static str),
        Completed(RuntimeValue),
        Failed(&'static str),
    }

    struct FakePlan {
        expected: RuntimeRequest,
        context: &'static str,
        events: Vec<FakeEvent>,
        held: bool,
        acknowledge_cancellation: bool,
    }

    impl FakePlan {
        fn completes(expected: RuntimeRequest, context: &'static str, value: RuntimeValue) -> Self {
            Self {
                expected,
                context,
                events: vec![FakeEvent::Completed(value)],
                held: false,
                acknowledge_cancellation: false,
            }
        }

        fn held(expected: RuntimeRequest, events: Vec<FakeEvent>) -> Self {
            Self {
                expected,
                context: "operation-context",
                events,
                held: true,
                acknowledge_cancellation: false,
            }
        }

        fn cancellation(expected: RuntimeRequest) -> Self {
            Self {
                expected,
                context: "operation-context",
                events: Vec::new(),
                held: true,
                acknowledge_cancellation: true,
            }
        }
    }

    struct ActiveFakeOperation {
        cancellation: Cancellation,
        events: Vec<FakeEvent>,
        held: bool,
        acknowledge_cancellation: bool,
    }

    #[derive(Default)]
    struct FakeRuntimeState {
        plans: VecDeque<FakePlan>,
        submitted: Vec<(OperationId, RuntimeRequest)>,
        cancellations: Vec<(OperationId, Cancellation)>,
        active: HashMap<OperationId, ActiveFakeOperation>,
        events: VecDeque<RuntimeEvent>,
        cancellation_requested: Vec<OperationId>,
        cancellation_observed: Vec<OperationId>,
        cancellation_acknowledged: Vec<OperationId>,
    }

    #[derive(Clone)]
    struct FakeRuntime {
        state: Rc<RefCell<FakeRuntimeState>>,
    }

    impl FakeRuntime {
        fn scripted(plans: Vec<FakePlan>) -> (Self, Rc<RefCell<FakeRuntimeState>>) {
            let state = Rc::new(RefCell::new(FakeRuntimeState {
                plans: plans.into(),
                ..FakeRuntimeState::default()
            }));
            (
                Self {
                    state: state.clone(),
                },
                state,
            )
        }

        fn release_ready(&self) {
            let mut state = self.state.borrow_mut();
            let ids = state.active.keys().copied().collect::<Vec<_>>();
            for id in ids {
                let requested = state
                    .active
                    .get(&id)
                    .is_some_and(|operation| operation.cancellation.is_requested());
                if requested && !state.cancellation_observed.contains(&id) {
                    state.cancellation_observed.push(id);
                }
                let acknowledge = requested
                    && state
                        .active
                        .get(&id)
                        .is_some_and(|operation| operation.acknowledge_cancellation);
                if acknowledge {
                    state.events.push_back(RuntimeEvent::Cancelled(id));
                    state.cancellation_acknowledged.push(id);
                    state.active.remove(&id);
                    continue;
                }
                let release = state
                    .active
                    .get(&id)
                    .is_some_and(|operation| !operation.held);
                if release {
                    let operation = state.active.remove(&id).unwrap();
                    for event in operation.events {
                        state.events.push_back(fake_event(id, event));
                    }
                }
            }
        }

        fn release_next_held_operation(&self) {
            if let Some(operation) = self
                .state
                .borrow_mut()
                .active
                .values_mut()
                .find(|operation| operation.held && !operation.events.is_empty())
            {
                operation.held = false;
            }
        }
    }

    fn fake_event(id: OperationId, event: FakeEvent) -> RuntimeEvent {
        match event {
            FakeEvent::Lifecycle(payload) => RuntimeEvent::Lifecycle(
                id,
                crate::events::AppEvent::WorkerOutput(crate::storage::db::LifecycleEvent {
                    id: 1,
                    timestamp: "2026-01-01T00:00:00Z".into(),
                    kind: "worker_output".into(),
                    task_id: Some("T-0001".into()),
                    run_id: Some(1),
                    agent_id: Some("codex-main".into()),
                    payload: Some(payload.into()),
                }),
            ),
            FakeEvent::Completed(value) => RuntimeEvent::Completed(id, Box::new(value)),
            FakeEvent::Failed(error) => RuntimeEvent::Failed(id, error.into()),
        }
    }

    impl RuntimePort for FakeRuntime {
        fn submit(&mut self, request: RuntimeRequest) -> (OperationId, Cancellation) {
            let mut state = self.state.borrow_mut();
            let plan = state.plans.pop_front().expect("unexpected runtime request");
            assert_eq!(request, plan.expected);
            let id = OperationId(state.submitted.len() as u64 + 1);
            let cancellation = Cancellation::new();
            state.submitted.push((id, request));
            state.cancellations.push((id, cancellation.clone()));
            state.events.push_back(RuntimeEvent::Started(id));
            state.events.push_back(RuntimeEvent::Context(
                id,
                SessionContext {
                    project: Some(plan.context.into()),
                },
            ));
            state.active.insert(
                id,
                ActiveFakeOperation {
                    cancellation: cancellation.clone(),
                    events: plan.events,
                    held: plan.held,
                    acknowledge_cancellation: plan.acknowledge_cancellation,
                },
            );
            drop(state);
            self.release_ready();
            (id, cancellation)
        }

        fn recv(&self) -> Result<RuntimeEvent, std::sync::mpsc::RecvError> {
            self.release_ready();
            self.state
                .borrow_mut()
                .events
                .pop_front()
                .ok_or(std::sync::mpsc::RecvError)
        }

        fn try_recv(&self) -> Result<RuntimeEvent, std::sync::mpsc::TryRecvError> {
            self.release_next_held_operation();
            self.release_ready();
            self.state
                .borrow_mut()
                .events
                .pop_front()
                .ok_or(std::sync::mpsc::TryRecvError::Empty)
        }

        fn cancel(&self, cancellation: &Cancellation) {
            cancellation.request();
            let mut state = self.state.borrow_mut();
            let id = state
                .cancellations
                .iter()
                .find_map(|(id, handle)| handle.is_requested().then_some(*id))
                .expect("unknown cancellation handle");
            state.cancellation_requested.push(id);
        }
    }

    fn initial_context(project: &'static str) -> FakePlan {
        FakePlan::completes(
            RuntimeRequest::ProjectStatus,
            project,
            RuntimeValue::Status("context ready".into()),
        )
    }

    fn refreshed_context(project: &'static str) -> FakePlan {
        initial_context(project)
    }

    fn run_fake(
        keys: Vec<Key>,
        timed_keys: Vec<Option<Key>>,
        plans: Vec<FakePlan>,
    ) -> (String, Rc<RefCell<FakeRuntimeState>>) {
        let (runtime, state) = FakeRuntime::scripted(plans);
        let terminal = FakeTerminal {
            keys,
            timed_keys,
            output: String::new(),
        };
        let mut editor = Editor::new_with_runtime(terminal, runtime);
        editor.run().unwrap();
        assert!(state.borrow().plans.is_empty());
        (editor.terminal.output, state)
    }

    fn dispatch_two_candidates() -> (String, Rc<RefCell<FakeRuntimeState>>) {
        run_fake(
            command_keys("dispatch T-0001"),
            vec![Some(Key::Down), Some(Key::Enter)],
            vec![
                initial_context("initial-project"),
                FakePlan::completes(
                    RuntimeRequest::DispatchCandidates("T-0001".into()),
                    "candidate-context",
                    RuntimeValue::AgentCandidates {
                        task_id: "T-0001".into(),
                        agents: vec!["codex-main".into(), "codex-secondary".into()],
                    },
                ),
                FakePlan::completes(
                    RuntimeRequest::Dispatch {
                        task_id: "T-0001".into(),
                        agent_id: Some("codex-secondary".into()),
                    },
                    "dispatch-context",
                    RuntimeValue::Status("dispatch complete".into()),
                ),
                refreshed_context("after-dispatch"),
            ],
        )
    }

    #[test]
    fn interactive_cancel_yes_submits_cancel_task() {
        let (output, state) = run_fake(
            command_keys("cancel T-0001")
                .into_iter()
                .take_while(|key| *key != Key::Eof)
                .chain([Key::Char('y'), Key::Eof])
                .collect(),
            Vec::new(),
            vec![
                initial_context("initial-project"),
                FakePlan::completes(
                    RuntimeRequest::CancelTask("T-0001".into()),
                    "operation-context",
                    RuntimeValue::Cancelled(true),
                ),
                refreshed_context("after-task-cancel"),
            ],
        );
        let submitted = &state.borrow().submitted;
        assert_eq!(submitted[0].1, RuntimeRequest::ProjectStatus);
        assert_eq!(submitted[1].1, RuntimeRequest::CancelTask("T-0001".into()));
        assert_eq!(submitted[2].1, RuntimeRequest::ProjectStatus);
        assert_eq!(
            submitted
                .iter()
                .filter(|(_, request)| matches!(request, RuntimeRequest::CancelTask(_)))
                .count(),
            1
        );
        assert!(output.contains("confirm task cancellation? [y/N] "));
        assert!(output.contains("project: after-task-cancel\r\n"));
        assert!(output.ends_with("orc> "));
    }

    #[test]
    fn interactive_cancel_no_does_not_cancel_task() {
        let (output, state) = run_fake(
            command_keys("cancel T-0001")
                .into_iter()
                .take_while(|key| *key != Key::Eof)
                .chain([Key::Char('n'), Key::Eof])
                .collect(),
            Vec::new(),
            vec![initial_context("initial-project")],
        );
        assert_eq!(state.borrow().submitted.len(), 1);
        assert!(
            !state
                .borrow()
                .submitted
                .iter()
                .any(|(_, request)| matches!(request, RuntimeRequest::CancelTask(_)))
        );
        assert!(output.contains("cancelled; no task state changed"));
        assert!(output.ends_with("orc> "));
    }

    #[test]
    fn dispatch_multiple_candidates_enters_selection() {
        let (output, _) = dispatch_two_candidates();
        assert!(output.contains("select dispatch agent:\r\n> codex-main\r\n  codex-secondary\r\n"));
    }

    #[test]
    fn dispatch_selection_routes_selected_agent() {
        let (_, state) = dispatch_two_candidates();
        assert!(state.borrow().submitted.iter().any(|(_, request)| request
            == &RuntimeRequest::Dispatch {
                task_id: "T-0001".into(),
                agent_id: Some("codex-secondary".into())
            }));
    }

    #[test]
    fn dispatch_single_candidate_skips_selection() {
        let (output, state) = run_fake(
            command_keys("dispatch T-0001"),
            vec![Some(Key::Enter)],
            vec![
                initial_context("initial-project"),
                FakePlan::completes(
                    RuntimeRequest::DispatchCandidates("T-0001".into()),
                    "candidate-context",
                    RuntimeValue::AgentCandidates {
                        task_id: "T-0001".into(),
                        agents: vec!["codex-main".into()],
                    },
                ),
                FakePlan::completes(
                    RuntimeRequest::Dispatch {
                        task_id: "T-0001".into(),
                        agent_id: Some("codex-main".into()),
                    },
                    "dispatch-context",
                    RuntimeValue::Status("dispatch complete".into()),
                ),
                refreshed_context("after-dispatch"),
            ],
        );
        assert!(!output.contains("select dispatch agent"));
        assert_eq!(
            state.borrow().submitted[2].1,
            RuntimeRequest::Dispatch {
                task_id: "T-0001".into(),
                agent_id: Some("codex-main".into())
            }
        );
    }

    #[test]
    fn dispatch_explicit_agent_bypasses_selection() {
        let (output, state) = run_fake(
            command_keys("dispatch T-0001 codex-secondary"),
            Vec::new(),
            vec![
                initial_context("initial-project"),
                FakePlan::completes(
                    RuntimeRequest::Dispatch {
                        task_id: "T-0001".into(),
                        agent_id: Some("codex-secondary".into()),
                    },
                    "dispatch-context",
                    RuntimeValue::Status("dispatch complete".into()),
                ),
                refreshed_context("after-explicit-dispatch"),
            ],
        );
        assert!(!output.contains("select dispatch agent"));
        assert!(
            !state
                .borrow()
                .submitted
                .iter()
                .any(|(_, request)| matches!(request, RuntimeRequest::DispatchCandidates(_)))
        );
    }

    #[test]
    fn dispatch_no_candidates_reports_error() {
        let (output, _) = run_fake(
            command_keys("dispatch T-0001"),
            Vec::new(),
            vec![
                initial_context("initial-project"),
                FakePlan::completes(
                    RuntimeRequest::DispatchCandidates("T-0001".into()),
                    "candidate-context",
                    RuntimeValue::AgentCandidates {
                        task_id: "T-0001".into(),
                        agents: Vec::new(),
                    },
                ),
                refreshed_context("after-zero-candidates"),
            ],
        );
        assert!(output.contains("error: no eligible agent found"));
        assert!(output.contains("project: after-zero-candidates"));
        assert!(output.ends_with("orc> "));

        let (output, _) = run_fake(
            command_keys("dispatch T-0001"),
            Vec::new(),
            vec![
                initial_context("initial-project"),
                FakePlan {
                    expected: RuntimeRequest::DispatchCandidates("T-0001".into()),
                    context: "candidate-context",
                    events: vec![FakeEvent::Failed("candidate lookup failed")],
                    held: false,
                    acknowledge_cancellation: false,
                },
                refreshed_context("after-candidate-error"),
            ],
        );
        assert!(output.contains("error: candidate lookup failed"));
        assert!(output.contains("project: after-candidate-error"));
        assert!(output.ends_with("orc> "));
    }

    #[test]
    fn context_refreshes_after_success() {
        assert_refresh_after(
            "tasks",
            FakePlan::completes(
                RuntimeRequest::Tasks,
                "operation-context",
                RuntimeValue::Tasks(Vec::new()),
            ),
            Vec::new(),
            "after-success",
        );
    }

    #[test]
    fn context_refreshes_after_failure() {
        assert_refresh_after(
            "task T-0001",
            FakePlan {
                expected: RuntimeRequest::TaskShow("T-0001".into()),
                context: "operation-context",
                events: vec![FakeEvent::Failed("controlled failure")],
                held: false,
                acknowledge_cancellation: false,
            },
            Vec::new(),
            "after-failure",
        );
    }

    #[test]
    fn context_refreshes_after_cancellation() {
        let (output, state) = run_fake(
            command_keys("dispatch T-0001 codex-main"),
            vec![Some(Key::CtrlC)],
            vec![
                initial_context("initial-project"),
                FakePlan::cancellation(RuntimeRequest::Dispatch {
                    task_id: "T-0001".into(),
                    agent_id: Some("codex-main".into()),
                }),
                refreshed_context("after-cancellation"),
            ],
        );
        assert_context_requests(&state.borrow(), 2);
        assert!(output.contains("project: after-cancellation\r\n"));
    }

    #[test]
    fn context_refreshes_after_dispatch() {
        let (output, state) = run_fake(
            command_keys("dispatch T-0001 codex-secondary"),
            Vec::new(),
            vec![
                initial_context("initial-project"),
                FakePlan::completes(
                    RuntimeRequest::Dispatch {
                        task_id: "T-0001".into(),
                        agent_id: Some("codex-secondary".into()),
                    },
                    "operation-context",
                    RuntimeValue::Status("dispatched".into()),
                ),
                refreshed_context("after-dispatch"),
            ],
        );
        assert_context_requests(&state.borrow(), 2);
        assert!(output.contains("project: after-dispatch\r\n"));
    }

    #[test]
    fn context_refreshes_after_confirmed_task_cancel() {
        let (output, state) = run_fake(
            command_keys("cancel T-0001")
                .into_iter()
                .take_while(|key| *key != Key::Eof)
                .chain([Key::Char('y'), Key::Eof])
                .collect(),
            Vec::new(),
            vec![
                initial_context("initial-project"),
                FakePlan::completes(
                    RuntimeRequest::CancelTask("T-0001".into()),
                    "operation-context",
                    RuntimeValue::Cancelled(true),
                ),
                refreshed_context("after-confirmed-cancel"),
            ],
        );
        assert_context_requests(&state.borrow(), 2);
        assert!(output.contains("project: after-confirmed-cancel\r\n"));
    }

    #[test]
    fn lifecycle_stream_precedes_operation_completion() {
        let (output, _) = run_fake(
            command_keys("dispatch T-0001 codex-main"),
            Vec::new(),
            vec![
                initial_context("initial-project"),
                FakePlan::held(
                    RuntimeRequest::Dispatch {
                        task_id: "T-0001".into(),
                        agent_id: Some("codex-main".into()),
                    },
                    vec![
                        FakeEvent::Lifecycle("building artifact"),
                        FakeEvent::Completed(RuntimeValue::Status("operation complete".into())),
                    ],
                ),
                refreshed_context("after-lifecycle"),
            ],
        );
        let lifecycle = output.find("progress: worker: building artifact").unwrap();
        let completion = output.find("success: operation complete").unwrap();
        assert!(lifecycle < completion);
    }

    #[test]
    fn ctrl_c_reaches_active_operation_cancellation() {
        let (_, state) = run_fake(
            command_keys("dispatch T-0001 codex-main"),
            vec![Some(Key::CtrlC)],
            vec![
                initial_context("initial-project"),
                FakePlan::cancellation(RuntimeRequest::Dispatch {
                    task_id: "T-0001".into(),
                    agent_id: Some("codex-main".into()),
                }),
                refreshed_context("after-cancellation"),
            ],
        );
        let state = state.borrow();
        assert_eq!(state.cancellation_requested, vec![OperationId(2)]);
        assert_eq!(state.cancellation_observed, vec![OperationId(2)]);
        assert_eq!(state.cancellation_acknowledged, vec![OperationId(2)]);
        assert!(state.cancellations[1].1.is_requested());
    }

    #[test]
    fn cancelled_operation_recovers_prompt() {
        let (output, state) = run_fake(
            command_keys("dispatch T-0001 codex-main"),
            vec![Some(Key::CtrlC)],
            vec![
                initial_context("initial-project"),
                FakePlan::cancellation(RuntimeRequest::Dispatch {
                    task_id: "T-0001".into(),
                    agent_id: Some("codex-main".into()),
                }),
                refreshed_context("cancel-recovery-project"),
            ],
        );
        assert_eq!(
            state.borrow().cancellation_acknowledged,
            vec![OperationId(2)]
        );
        assert!(output.contains("operation cancelled\r\n"));
        assert!(output.contains("project: cancel-recovery-project\r\n"));
        assert!(output.ends_with("orc> "));
    }

    fn assert_context_requests(state: &FakeRuntimeState, expected: usize) {
        let requests = state
            .submitted
            .iter()
            .filter(|(_, request)| *request == RuntimeRequest::ProjectStatus)
            .count();
        assert_eq!(requests, expected);
        assert_eq!(
            state.submitted.first().unwrap().1,
            RuntimeRequest::ProjectStatus
        );
        assert_eq!(
            state.submitted.last().unwrap().1,
            RuntimeRequest::ProjectStatus
        );
    }

    fn assert_refresh_after(
        command: &str,
        operation: FakePlan,
        timed_keys: Vec<Option<Key>>,
        refreshed: &'static str,
    ) {
        let (output, state) = run_fake(
            command_keys(command),
            timed_keys,
            vec![
                initial_context("initial-project"),
                operation,
                refreshed_context(refreshed),
            ],
        );
        assert_context_requests(&state.borrow(), 2);
        assert!(output.contains(&format!("project: {refreshed}\r\n")));
        assert!(output.ends_with("orc> "));
    }
}
