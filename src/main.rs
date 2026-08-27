use argon2::{Algorithm, Argon2, Params, Version};
use std::env;
use std::fs::{self, File, OpenOptions};
#[cfg(target_os = "linux")]
use std::io::Read;
use std::io::{self, Write};
#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossterm::{
    cursor::{Hide, Show},
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame, Terminal as RatatuiTerminal,
};

#[cfg(target_os = "linux")]
mod framebuffer;
#[cfg(target_os = "linux")]
use framebuffer::Framebuffer;

const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(2);
#[cfg(target_os = "linux")]
const KDSETMODE: libc::c_ulong = 0x4B3A;
#[cfg(target_os = "linux")]
const KD_TEXT: libc::c_int = 0;
#[cfg(target_os = "linux")]
const KD_GRAPHICS: libc::c_int = 1;
const APP_NAME: &str = "QuietWrite";

#[derive(Debug)]
struct Document {
    text: Vec<char>,
    cursor: usize,
    dirty: bool,
    preferred_column: Option<usize>,
}

impl Document {
    fn from_string(text: String) -> Self {
        let text: Vec<char> = text.chars().collect();
        let cursor = text.len();
        Self {
            text,
            cursor,
            dirty: false,
            preferred_column: None,
        }
    }

    fn insert(&mut self, ch: char) {
        self.text.insert(self.cursor, ch);
        self.cursor += 1;
        self.dirty = true;
        self.preferred_column = None;
    }

    fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.text.remove(self.cursor);
            self.dirty = true;
            self.preferred_column = None;
        }
    }

    fn delete(&mut self) {
        if self.cursor < self.text.len() {
            self.text.remove(self.cursor);
            self.dirty = true;
            self.preferred_column = None;
        }
    }

    fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
        self.preferred_column = None;
    }
    fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.text.len());
        self.preferred_column = None;
    }

    fn line_start(&self, at: usize) -> usize {
        self.text[..at]
            .iter()
            .rposition(|c| *c == '\n')
            .map_or(0, |i| i + 1)
    }

    fn line_end(&self, at: usize) -> usize {
        self.text[at..]
            .iter()
            .position(|c| *c == '\n')
            .map_or(self.text.len(), |i| at + i)
    }

    fn home(&mut self) {
        self.cursor = self.line_start(self.cursor);
        self.preferred_column = None;
    }
    fn end(&mut self) {
        self.cursor = self.line_end(self.cursor);
        self.preferred_column = None;
    }

    fn move_visual_rows(&mut self, width: usize, rows: isize) {
        let ranges = wrapped_ranges(&self.text, width);
        let (current_row, current_column) = cursor_position(&self.text, &ranges, self.cursor);
        let preferred = self.preferred_column.unwrap_or(current_column);
        self.preferred_column = Some(preferred);
        let target_row = (current_row as isize + rows)
            .clamp(0, ranges.len().saturating_sub(1) as isize) as usize;
        let (start, end) = ranges[target_row];
        self.cursor = index_at_visual_column(&self.text, start, end, preferred);
    }

    fn word_count(&self) -> usize {
        self.text
            .iter()
            .collect::<String>()
            .split_whitespace()
            .count()
    }

    fn character_count(&self) -> usize {
        self.text.len()
    }

    fn as_string(&self) -> String {
        self.text.iter().collect()
    }
}

#[derive(Debug, PartialEq)]
enum Key {
    Char(char),
    Enter,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Save,
    New,
    Quit,
    Redraw,
    Help,
    Browser,
    Escape,
    Theme,
    Rotate,
    Larger,
    Smaller,
    Unknown,
}

#[cfg(any(target_os = "linux", test))]
fn decode_key(bytes: &mut Vec<u8>) -> Option<Key> {
    if bytes.is_empty() {
        return None;
    }
    let b = bytes[0];
    let control = match b {
        17 => Some(Key::Quit),
        19 => Some(Key::Save),
        14 => Some(Key::New),
        12 => Some(Key::Redraw),
        13 | 10 => Some(Key::Enter),
        8 | 127 => Some(Key::Backspace),
        _ => None,
    };
    if let Some(key) = control {
        bytes.remove(0);
        return Some(key);
    }
    if b == 27 {
        const SEQUENCES: &[(&[u8], Key)] = &[
            (b"\x1b[A", Key::Up),
            (b"\x1b[B", Key::Down),
            (b"\x1b[C", Key::Right),
            (b"\x1b[D", Key::Left),
            (b"\x1b[H", Key::Home),
            (b"\x1b[F", Key::End),
            (b"\x1b[1~", Key::Home),
            (b"\x1b[4~", Key::End),
            (b"\x1b[3~", Key::Delete),
            (b"\x1b[5~", Key::PageUp),
            (b"\x1b[6~", Key::PageDown),
            (b"\x1bOP", Key::Help),
            (b"\x1b[11~", Key::Help),
            (b"\x1b[[A", Key::Help),
            (b"\x1bOQ", Key::Browser),
            (b"\x1b[12~", Key::Browser),
            (b"\x1b[15~", Key::Theme),
            (b"\x1b[[E", Key::Theme),
            (b"\x1b[17~", Key::Rotate),
            (b"\x1b[18~", Key::Larger),
            (b"\x1b[19~", Key::Smaller),
        ];
        for (sequence, key) in SEQUENCES {
            if bytes.starts_with(sequence) {
                bytes.drain(..sequence.len());
                return Some(match key {
                    Key::Up => Key::Up,
                    Key::Down => Key::Down,
                    Key::Right => Key::Right,
                    Key::Left => Key::Left,
                    Key::Home => Key::Home,
                    Key::End => Key::End,
                    Key::Delete => Key::Delete,
                    Key::PageUp => Key::PageUp,
                    Key::PageDown => Key::PageDown,
                    Key::Help => Key::Help,
                    Key::Browser => Key::Browser,
                    Key::Theme => Key::Theme,
                    Key::Rotate => Key::Rotate,
                    Key::Larger => Key::Larger,
                    Key::Smaller => Key::Smaller,
                    _ => Key::Unknown,
                });
            }
            if sequence.starts_with(bytes.as_slice()) {
                return None;
            }
        }
        bytes.remove(0);
        return Some(Key::Unknown);
    }
    if b < 32 {
        bytes.remove(0);
        return Some(if b == 9 {
            Key::Char('\t')
        } else {
            Key::Unknown
        });
    }
    let length = if b < 0x80 {
        1
    } else if b & 0xe0 == 0xc0 {
        2
    } else if b & 0xf0 == 0xe0 {
        3
    } else if b & 0xf8 == 0xf0 {
        4
    } else {
        1
    };
    if bytes.len() < length {
        return None;
    }
    let result = std::str::from_utf8(&bytes[..length])
        .ok()
        .and_then(|s| s.chars().next())
        .map(Key::Char)
        .unwrap_or(Key::Unknown);
    bytes.drain(..length);
    Some(result)
}

fn char_width(ch: char, column: usize) -> usize {
    if ch == '\t' {
        return 4 - (column % 4);
    }
    if ch.is_control() {
        return 0;
    }
    // Common full-width ranges; enough for predictable terminal wrapping without a Unicode dependency.
    if matches!(ch as u32,
        0x1100..=0x115f | 0x2329..=0x232a | 0x2e80..=0xa4cf |
        0xac00..=0xd7a3 | 0xf900..=0xfaff | 0xfe10..=0xfe19 |
        0xfe30..=0xfe6f | 0xff00..=0xff60 | 0xffe0..=0xffe6 |
        0x1f300..=0x1faff | 0x20000..=0x3fffd)
    {
        2
    } else {
        1
    }
}

fn wrapped_ranges(text: &[char], width: usize) -> Vec<(usize, usize)> {
    let width = width.max(1);
    let mut ranges = Vec::new();
    let mut start = 0;
    let mut i = 0;
    let mut column = 0;
    while i < text.len() {
        if text[i] == '\n' {
            ranges.push((start, i));
            i += 1;
            start = i;
            column = 0;
            continue;
        }
        let w = char_width(text[i], column);
        if column + w > width && i > start {
            ranges.push((start, i));
            start = i;
            column = 0;
            continue;
        }
        column += w;
        i += 1;
    }
    ranges.push((start, text.len()));
    ranges
}

fn cursor_position(text: &[char], ranges: &[(usize, usize)], cursor: usize) -> (usize, usize) {
    let mut found = (0, 0);
    for (row, &(start, end)) in ranges.iter().enumerate() {
        if start <= cursor && cursor <= end {
            let mut column = 0;
            for ch in &text[start..cursor] {
                column += char_width(*ch, column);
            }
            found = (row, column);
        }
    }
    found
}

fn index_at_visual_column(text: &[char], start: usize, end: usize, target: usize) -> usize {
    let mut index = start;
    let mut column = 0;
    while index < end {
        let width = char_width(text[index], column);
        if column + width > target {
            break;
        }
        column += width;
        index += 1;
    }
    index
}

#[cfg(target_os = "linux")]
struct Terminal {
    original: libc::termios,
    fd: i32,
}

#[cfg(target_os = "linux")]
impl Terminal {
    fn enter() -> io::Result<Self> {
        let fd = io::stdin().as_raw_fd();
        if unsafe { libc::isatty(fd) } != 1 {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "stdin is not a terminal",
            ));
        }
        let mut original = unsafe { std::mem::zeroed::<libc::termios>() };
        if unsafe { libc::tcgetattr(fd, &mut original) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut raw = original;
        raw.c_iflag &= !(libc::BRKINT | libc::ICRNL | libc::INPCK | libc::ISTRIP | libc::IXON);
        raw.c_oflag &= !libc::OPOST;
        raw.c_cflag |= libc::CS8;
        raw.c_lflag &= !(libc::ECHO | libc::ICANON | libc::IEXTEN | libc::ISIG);
        raw.c_cc[libc::VMIN] = 0;
        raw.c_cc[libc::VTIME] = 1;
        if unsafe { libc::tcsetattr(fd, libc::TCSAFLUSH, &raw) } != 0 {
            return Err(io::Error::last_os_error());
        }
        unsafe {
            libc::ioctl(fd, KDSETMODE as _, KD_GRAPHICS);
        }
        Ok(Self { original, fd })
    }

    #[allow(dead_code)]
    fn size(&self) -> (usize, usize) {
        let mut size = unsafe { std::mem::zeroed::<libc::winsize>() };
        if unsafe { libc::ioctl(self.fd, libc::TIOCGWINSZ, &mut size) } == 0 && size.ws_col > 0 {
            (size.ws_col as usize, size.ws_row as usize)
        } else {
            (80, 24)
        }
    }
}

#[cfg(target_os = "linux")]
impl Drop for Terminal {
    fn drop(&mut self) {
        unsafe {
            libc::tcsetattr(self.fd, libc::TCSAFLUSH, &self.original);
        }
        unsafe {
            libc::ioctl(self.fd, KDSETMODE as _, KD_TEXT);
        }
    }
}

#[derive(Clone, Copy)]
struct ThemePalette {
    name: &'static str,
    background: (u8, u8, u8),
    foreground: (u8, u8, u8),
    muted: (u8, u8, u8),
    accent: (u8, u8, u8),
    status: (u8, u8, u8),
}

const THEMES: &[ThemePalette] = &[
    ThemePalette {
        name: "Moon Ink",
        background: (17, 23, 31),
        foreground: (238, 241, 245),
        muted: (168, 178, 190),
        accent: (88, 166, 255),
        status: (31, 43, 57),
    },
    ThemePalette {
        name: "Campfire",
        background: (33, 24, 20),
        foreground: (252, 237, 216),
        muted: (198, 164, 128),
        accent: (255, 139, 76),
        status: (61, 41, 31),
    },
    ThemePalette {
        name: "Moss Garden",
        background: (15, 29, 24),
        foreground: (229, 243, 224),
        muted: (145, 181, 151),
        accent: (121, 211, 133),
        status: (29, 52, 42),
    },
    ThemePalette {
        name: "Berry Jam",
        background: (31, 20, 40),
        foreground: (247, 233, 250),
        muted: (190, 158, 201),
        accent: (240, 112, 196),
        status: (57, 35, 68),
    },
];

const SAVE_MESSAGES: &[&str] = &["ink safe", "tucked away", "captured", "saved ✦", "all cozy"];
const BLANK_PROMPTS: &[&str] = &[
    "Start anywhere.",
    "Make a little mess.",
    "One true sentence.",
    "What wants saying?",
    "Begin before ready.",
];
const CATEGORIES: &[(&str, &str)] = &[
    ("Notes", "Notes"),
    ("Poems", "Poems"),
    ("Secret Thoughts", "Secret Thoughts"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Editor,
    Categories,
    Documents,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LockPrompt {
    Create,
    Confirm,
    Unlock,
}

struct App {
    document: Document,
    path: PathBuf,
    directory: PathBuf,
    scroll: usize,
    message: String,
    last_save: Instant,
    rotation: u8,
    zoom: i8,
    theme: u8,
    display_config: PathBuf,
    save_count: usize,
    help_visible: bool,
    screen: Screen,
    category_index: usize,
    document_index: usize,
    documents: Vec<PathBuf>,
    lock_prompt: Option<LockPrompt>,
    password_input: String,
    password_confirmation: String,
    secret_unlocked: bool,
    secret_lock_path: PathBuf,
}

impl App {
    fn open(directory: PathBuf, explicit: Option<PathBuf>, force_new: bool) -> io::Result<Self> {
        fs::create_dir_all(&directory)?;
        let explicit_open = explicit.is_some();
        let path = if let Some(path) = explicit {
            path
        } else if force_new {
            let notes = directory.join(CATEGORIES[0].1);
            fs::create_dir_all(&notes)?;
            new_note_path(&notes)
        } else {
            latest_note(&directory).unwrap_or_else(|| new_note_path(&directory))
        };
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error),
        };
        let display_config = default_config_path();
        let secret_lock_path = default_secret_lock_path();
        let (rotation, zoom, theme) = read_display_config(&display_config);
        let screen = if explicit_open || force_new {
            Screen::Editor
        } else {
            Screen::Categories
        };
        Ok(Self {
            document: Document::from_string(text),
            path,
            directory,
            scroll: 0,
            message: "fresh page".into(),
            last_save: Instant::now(),
            rotation,
            zoom,
            theme,
            display_config,
            save_count: 0,
            help_visible: false,
            screen,
            category_index: 0,
            document_index: 0,
            documents: Vec::new(),
            lock_prompt: None,
            password_input: String::new(),
            password_confirmation: String::new(),
            secret_unlocked: false,
            secret_lock_path,
        })
    }

    fn save(&mut self) -> io::Result<()> {
        if !self.document.dirty {
            self.save_count = self.save_count.wrapping_add(1);
            self.message = SAVE_MESSAGES[self.save_count % SAVE_MESSAGES.len()].into();
            self.last_save = Instant::now();
            return Ok(());
        }
        atomic_write(&self.path, self.document.as_string().as_bytes())?;
        self.document.dirty = false;
        self.last_save = Instant::now();
        self.save_count = self.save_count.wrapping_add(1);
        self.message = SAVE_MESSAGES[self.save_count % SAVE_MESSAGES.len()].into();
        Ok(())
    }

    fn new_note(&mut self) -> io::Result<()> {
        self.save()?;
        let directory = self.category_directory(self.category_index);
        fs::create_dir_all(&directory)?;
        self.path = new_note_path(&directory);
        self.document = Document::from_string(String::new());
        self.document.dirty = true;
        self.scroll = 0;
        self.message = "New note".into();
        self.screen = Screen::Editor;
        Ok(())
    }

    fn category_directory(&self, index: usize) -> PathBuf {
        self.directory
            .join(CATEGORIES[index.min(CATEGORIES.len() - 1)].1)
    }

    fn refresh_documents(&mut self) {
        self.documents = notes_for_category(&self.directory, self.category_index);
        self.document_index = self
            .document_index
            .min(self.documents.len().saturating_sub(1));
    }

    fn begin_secret_unlock(&mut self) {
        self.password_input.clear();
        self.password_confirmation.clear();
        self.lock_prompt = Some(if self.secret_lock_path.exists() {
            LockPrompt::Unlock
        } else {
            LockPrompt::Create
        });
        self.message = if self.secret_lock_path.exists() {
            "Enter Secret Thoughts password".into()
        } else {
            "Create a Secret Thoughts password".into()
        };
    }

    fn open_category(&mut self) {
        self.document_index = 0;
        self.refresh_documents();
        self.screen = Screen::Documents;
    }

    fn submit_lock_prompt(&mut self) {
        match self.lock_prompt {
            Some(LockPrompt::Create) => {
                if self.password_input.len() < 4 {
                    self.message = "Use at least 4 characters".into();
                    return;
                }
                self.password_confirmation = std::mem::take(&mut self.password_input);
                self.lock_prompt = Some(LockPrompt::Confirm);
                self.message = "Enter it again".into();
            }
            Some(LockPrompt::Confirm) => {
                if self.password_input != self.password_confirmation {
                    self.password_input.clear();
                    self.password_confirmation.clear();
                    self.lock_prompt = Some(LockPrompt::Create);
                    self.message = "Passwords did not match — try again".into();
                    return;
                }
                match write_secret_lock(&self.secret_lock_path, self.password_input.as_bytes()) {
                    Ok(()) => {
                        self.secret_unlocked = true;
                        self.lock_prompt = None;
                        self.password_input.clear();
                        self.password_confirmation.clear();
                        self.open_category();
                    }
                    Err(error) => self.message = format!("Could not save lock: {error}"),
                }
            }
            Some(LockPrompt::Unlock) => {
                match verify_secret_lock(&self.secret_lock_path, self.password_input.as_bytes()) {
                    Ok(true) => {
                        self.secret_unlocked = true;
                        self.lock_prompt = None;
                        self.password_input.clear();
                        self.open_category();
                    }
                    Ok(false) => {
                        self.password_input.clear();
                        self.message = "Incorrect password".into();
                    }
                    Err(error) => self.message = format!("Could not read lock: {error}"),
                }
            }
            None => {}
        }
    }

    fn open_selected_document(&mut self) -> io::Result<()> {
        let Some(path) = self.documents.get(self.document_index).cloned() else {
            return self.new_note();
        };
        self.save()?;
        let text = fs::read_to_string(&path)?;
        self.path = path;
        self.document = Document::from_string(text);
        self.scroll = 0;
        self.message = "opened".into();
        self.screen = Screen::Editor;
        Ok(())
    }

    #[allow(dead_code)]
    #[cfg(target_os = "linux")]
    fn render_terminal(&mut self, terminal: &Terminal) -> io::Result<()> {
        let (columns, rows) = terminal.size();
        if self.help_visible {
            let lines = [
                "QuietWrite keys",
                "",
                "Ctrl+S  Save",
                "Ctrl+N  New note",
                "Ctrl+Q  Save & quit",
                "F5      Change theme",
                "F6      Rotate",
                "F7      Larger text",
                "F8      Smaller text",
                "",
                "F1      Close help",
            ];
            let mut frame = String::from("\x1b[H\x1b[2J\x1b[?25l");
            for (index, line) in lines.iter().enumerate() {
                if index >= rows {
                    break;
                }
                frame.push_str(&truncate(line, columns));
                frame.push_str("\r\n");
            }
            let mut stdout = io::stdout().lock();
            stdout.write_all(frame.as_bytes())?;
            return stdout.flush();
        }
        let margin = if columns >= 60 { 2 } else { 1 };
        let content_width = columns.saturating_sub(margin * 2).max(1);
        let content_height = rows.saturating_sub(2).max(1);
        let ranges = wrapped_ranges(&self.document.text, content_width);
        let (cursor_row, cursor_column) =
            cursor_position(&self.document.text, &ranges, self.document.cursor);
        if cursor_row < self.scroll {
            self.scroll = cursor_row;
        }
        if cursor_row >= self.scroll + content_height {
            self.scroll = cursor_row + 1 - content_height;
        }

        let mut frame = String::with_capacity(columns * rows + rows * 8);
        frame.push_str("\x1b[H\x1b[0m");
        for screen_row in 0..content_height {
            frame.push_str("\x1b[2K");
            frame.push_str(&" ".repeat(margin));
            if let Some(&(start, end)) = ranges.get(self.scroll + screen_row) {
                let mut column = 0;
                for &ch in &self.document.text[start..end] {
                    match ch {
                        '\t' => {
                            let spaces = char_width(ch, column);
                            frame.push_str(&" ".repeat(spaces));
                            column += spaces;
                        }
                        ch if ch.is_control() => {}
                        ch => {
                            frame.push(ch);
                            column += char_width(ch, column);
                        }
                    }
                }
            }
            frame.push_str("\r\n");
        }
        let name = self
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("note.md");
        let state = if self.document.dirty {
            "writing"
        } else {
            "saved"
        };
        let status = format!(
            " {}  •  {} words  •  {} chars  •  {}  •  {} ",
            name,
            self.document.word_count(),
            self.document.character_count(),
            state,
            self.message
        );
        frame.push_str("\x1b[2K\x1b[7m");
        frame.push_str(&truncate(&status, columns));
        let used = visible_width(&truncate(&status, columns));
        if used < columns {
            frame.push_str(&" ".repeat(columns - used));
        }
        frame.push_str("\x1b[0m\r\n\x1b[2K");
        let help = " F1 Help  ·  F5 Theme  ·  F6 Rotate ";
        frame.push_str(&truncate(help, columns));

        let screen_cursor_row = cursor_row
            .saturating_sub(self.scroll)
            .min(content_height - 1);
        let screen_cursor_column = (margin + cursor_column).min(columns.saturating_sub(1));
        frame.push_str(&format!(
            "\x1b[{};{}H\x1b[?25h",
            screen_cursor_row + 1,
            screen_cursor_column + 1
        ));
        let mut stdout = io::stdout().lock();
        stdout.write_all(frame.as_bytes())?;
        stdout.flush()
    }

    #[cfg(target_os = "linux")]
    fn render_framebuffer(&mut self, framebuffer: &mut Framebuffer) -> io::Result<()> {
        let palette = THEMES[self.theme as usize % THEMES.len()];
        if self.screen != Screen::Editor {
            let layout = framebuffer.layout(self.rotation, -3);
            framebuffer.clear(palette.background, self.rotation);
            framebuffer.text(
                layout.margin_x,
                layout.margin_y,
                "QuietWrite",
                layout,
                palette.accent,
                palette.background,
                self.rotation,
            );
            let title = if self.screen == Screen::Categories {
                "Choose a shelf"
            } else {
                CATEGORIES[self.category_index].0
            };
            framebuffer.text(
                layout.margin_x,
                layout.margin_y + layout.line_height * 2,
                title,
                layout,
                palette.muted,
                palette.background,
                self.rotation,
            );
            if let Some(prompt) = self.lock_prompt {
                let heading = match prompt {
                    LockPrompt::Create => "Create Secret Thoughts password",
                    LockPrompt::Confirm => "Confirm password",
                    LockPrompt::Unlock => "Unlock Secret Thoughts",
                };
                let bullets = "*".repeat(self.password_input.chars().count());
                framebuffer.text(
                    layout.margin_x,
                    layout.margin_y + layout.line_height * 4,
                    heading,
                    layout,
                    palette.foreground,
                    palette.background,
                    self.rotation,
                );
                framebuffer.text(
                    layout.margin_x,
                    layout.margin_y + layout.line_height * 6,
                    &bullets,
                    layout,
                    palette.accent,
                    palette.background,
                    self.rotation,
                );
                framebuffer.text(
                    layout.margin_x,
                    layout.margin_y + layout.line_height * 8,
                    &self.message,
                    layout,
                    palette.muted,
                    palette.background,
                    self.rotation,
                );
                framebuffer.text(
                    layout.margin_x,
                    layout.logical_height.saturating_sub(layout.line_height * 2),
                    "Enter continue · F2 cancel",
                    layout,
                    palette.muted,
                    palette.background,
                    self.rotation,
                );
                framebuffer.flush();
                return Ok(());
            }
            let entries: Vec<String> = if self.screen == Screen::Categories {
                CATEGORIES
                    .iter()
                    .map(|(name, _)| (*name).to_string())
                    .collect()
            } else if self.documents.is_empty() {
                vec!["No writing yet — Enter creates one".into()]
            } else {
                self.documents
                    .iter()
                    .map(|path| {
                        path.file_stem()
                            .and_then(|name| name.to_str())
                            .unwrap_or("Untitled")
                            .to_string()
                    })
                    .collect()
            };
            let selected = if self.screen == Screen::Categories {
                self.category_index
            } else {
                self.document_index
            };
            for (index, entry) in entries
                .iter()
                .take(layout.content_rows.saturating_sub(5))
                .enumerate()
            {
                let y = layout.margin_y + layout.line_height * (4 + index);
                if index == selected {
                    framebuffer.rect(
                        layout.margin_x / 2,
                        y,
                        layout.logical_width.saturating_sub(layout.margin_x),
                        layout.line_height,
                        palette.status,
                        self.rotation,
                    );
                }
                let prefix = if index == selected { "› " } else { "  " };
                framebuffer.text(
                    layout.margin_x,
                    y,
                    &format!("{prefix}{entry}"),
                    layout,
                    if index == selected {
                        palette.foreground
                    } else {
                        palette.muted
                    },
                    if index == selected {
                        palette.status
                    } else {
                        palette.background
                    },
                    self.rotation,
                );
            }
            let footer = if self.screen == Screen::Categories {
                "↑↓ choose · Enter open · F5 theme · F7/F8 size"
            } else {
                "↑↓ choose · Enter open · Ctrl+N new · F2 back"
            };
            framebuffer.text(
                layout.margin_x,
                layout.logical_height.saturating_sub(layout.line_height * 2),
                footer,
                layout,
                palette.muted,
                palette.background,
                self.rotation,
            );
            framebuffer.flush();
            return Ok(());
        }
        if self.help_visible {
            let layout = framebuffer.layout(self.rotation, -4);
            framebuffer.clear(palette.background, self.rotation);
            let panel_x = layout.margin_x / 2;
            let panel_y = layout.margin_y / 2;
            let panel_width = layout.logical_width.saturating_sub(layout.margin_x);
            let lines = [
                ("QuietWrite keys", palette.accent),
                ("Ctrl+S   Save", palette.foreground),
                ("Ctrl+N   New note", palette.foreground),
                ("Ctrl+Q   Save & quit", palette.foreground),
                ("F2       Browse writing", palette.foreground),
                ("F5       Change theme", palette.foreground),
                ("F6       Rotate screen", palette.foreground),
                ("F7       Larger text", palette.foreground),
                ("F8       Smaller text", palette.foreground),
                ("F1       Close help", palette.muted),
            ];
            framebuffer.rect(
                panel_x,
                panel_y,
                panel_width,
                (lines.len() + 1) * layout.line_height,
                palette.status,
                self.rotation,
            );
            framebuffer.rect(
                panel_x,
                panel_y,
                (layout.cell_width / 7).max(4),
                (lines.len() + 1) * layout.line_height,
                palette.accent,
                self.rotation,
            );
            for (row, (line, color)) in lines.iter().enumerate() {
                framebuffer.text(
                    layout.margin_x,
                    panel_y + row * layout.line_height,
                    line,
                    layout,
                    *color,
                    palette.status,
                    self.rotation,
                );
            }
            framebuffer.flush();
            return Ok(());
        }
        let layout = framebuffer.layout(self.rotation, self.zoom);
        let ranges = wrapped_ranges(&self.document.text, layout.columns);
        let (cursor_row, cursor_column) =
            cursor_position(&self.document.text, &ranges, self.document.cursor);
        if cursor_row < self.scroll {
            self.scroll = cursor_row;
        }
        if cursor_row >= self.scroll + layout.content_rows {
            self.scroll = cursor_row + 1 - layout.content_rows;
        }

        framebuffer.clear(palette.background, self.rotation);
        let rail_width = (layout.cell_width / 8).max(4);
        framebuffer.rect(
            layout.margin_x / 3,
            layout.margin_y,
            rail_width,
            layout.content_rows * layout.line_height,
            palette.accent,
            self.rotation,
        );
        let dot = (layout.cell_width / 5).max(5);
        for index in 0..3 {
            framebuffer.rect(
                layout
                    .logical_width
                    .saturating_sub(layout.margin_x + dot * (index + 1) * 2),
                layout.margin_y / 2,
                dot,
                dot,
                palette.accent,
                self.rotation,
            );
        }
        if self.document.text.is_empty() {
            let seed = self
                .path
                .to_string_lossy()
                .bytes()
                .fold(0_usize, |sum, byte| sum.wrapping_add(byte as usize));
            let prompt = BLANK_PROMPTS[seed % BLANK_PROMPTS.len()];
            let prompt_width = visible_width(prompt) * layout.cell_width;
            let prompt_x = layout.logical_width.saturating_sub(prompt_width) / 2;
            let prompt_y = layout.margin_y + layout.line_height * (layout.content_rows / 3);
            framebuffer.text(
                prompt_x,
                prompt_y,
                prompt,
                layout,
                palette.muted,
                palette.background,
                self.rotation,
            );
        }
        for screen_row in 0..layout.content_rows {
            if let Some(&(start, end)) = ranges.get(self.scroll + screen_row) {
                let text: String = self.document.text[start..end]
                    .iter()
                    .filter(|ch| !ch.is_control() || **ch == '\t')
                    .collect();
                framebuffer.text(
                    layout.margin_x,
                    layout.margin_y + screen_row * layout.line_height,
                    &text,
                    layout,
                    palette.foreground,
                    palette.background,
                    self.rotation,
                );
            }
        }

        let screen_cursor_row = cursor_row
            .saturating_sub(self.scroll)
            .min(layout.content_rows - 1);
        let cursor_x = layout.margin_x + cursor_column * layout.cell_width;
        let cursor_y = layout.margin_y + screen_cursor_row * layout.line_height;
        framebuffer.rect(
            cursor_x,
            cursor_y + 3,
            (layout.cell_width / 10).max(3),
            layout.line_height.saturating_sub(6),
            palette.accent,
            self.rotation,
        );

        let status_y = layout.margin_y + layout.content_rows * layout.line_height;
        framebuffer.rect(
            0,
            status_y,
            layout.logical_width,
            layout.line_height,
            palette.status,
            self.rotation,
        );
        let name = self
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("note.md");
        let status_text = if layout.columns < 36 {
            format!(
                " ✦ {}w · {}c · {} ",
                self.document.word_count(),
                self.document.character_count(),
                if self.document.dirty {
                    "making magic"
                } else {
                    &self.message
                }
            )
        } else {
            format!(
                " ✦ {} · {} words · {} chars · {} ",
                name,
                self.document.word_count(),
                self.document.character_count(),
                if self.document.dirty {
                    "making magic"
                } else {
                    &self.message
                }
            )
        };
        let status = truncate(&status_text, layout.columns);
        framebuffer.text(
            layout.margin_x,
            status_y,
            &status,
            layout,
            palette.foreground,
            palette.status,
            self.rotation,
        );

        let help_y = status_y + layout.line_height;
        let help_text = if layout.columns < 36 {
            " F1 help · F5 · F6 "
        } else {
            " F1 help · F5 theme · F6 rotate · F7 larger · F8 smaller "
        };
        let help = truncate(help_text, layout.columns);
        framebuffer.text(
            layout.margin_x,
            help_y,
            &help,
            layout,
            palette.muted,
            palette.background,
            self.rotation,
        );
        framebuffer.flush();
        Ok(())
    }

    fn save_display_config(&mut self) {
        let contents = format!(
            "rotation={}\nzoom={}\ntheme={}\n",
            self.rotation, self.zoom, self.theme
        );
        match atomic_write(&self.display_config, contents.as_bytes()) {
            Ok(()) => self.message = "Display saved".into(),
            Err(error) => self.message = format!("Display setting failed: {error}"),
        }
    }

    fn handle(&mut self, key: Key, page_height: usize, page_width: usize) -> io::Result<bool> {
        if self.lock_prompt.is_some() {
            match key {
                Key::Char(character) if !character.is_control() => {
                    self.password_input.push(character)
                }
                Key::Backspace => {
                    self.password_input.pop();
                }
                Key::Enter => self.submit_lock_prompt(),
                Key::Escape | Key::Browser => {
                    self.password_input.clear();
                    self.password_confirmation.clear();
                    self.lock_prompt = None;
                    self.message = "Locked".into();
                }
                _ => {}
            }
            return Ok(true);
        }
        if self.screen != Screen::Editor {
            match key {
                Key::Quit => {
                    self.save()?;
                    return Ok(false);
                }
                Key::Theme => {
                    self.theme = (self.theme + 1) % THEMES.len() as u8;
                    self.save_display_config();
                    self.message = THEMES[self.theme as usize].name.into();
                }
                Key::Larger => {
                    self.zoom = (self.zoom + 1).min(5);
                    self.save_display_config();
                }
                Key::Smaller => {
                    self.zoom = (self.zoom - 1).max(-8);
                    self.save_display_config();
                }
                Key::Up => {
                    if self.screen == Screen::Categories {
                        self.category_index = self.category_index.saturating_sub(1);
                    } else {
                        self.document_index = self.document_index.saturating_sub(1);
                    }
                }
                Key::Down => {
                    if self.screen == Screen::Categories {
                        self.category_index = (self.category_index + 1).min(CATEGORIES.len() - 1);
                    } else if !self.documents.is_empty() {
                        self.document_index =
                            (self.document_index + 1).min(self.documents.len() - 1);
                    }
                }
                Key::Enter if self.screen == Screen::Categories => {
                    if self.category_index == 2 && !self.secret_unlocked {
                        self.begin_secret_unlock();
                    } else {
                        self.open_category();
                    }
                }
                Key::Enter if self.screen == Screen::Documents => self.open_selected_document()?,
                Key::New if self.screen == Screen::Documents => self.new_note()?,
                Key::Browser | Key::Escape if self.screen == Screen::Documents => {
                    if self.category_index == 2 {
                        self.secret_unlocked = false;
                    }
                    self.screen = Screen::Categories
                }
                Key::Browser | Key::Escape if self.screen == Screen::Categories => {
                    self.screen = Screen::Editor;
                }
                _ => {}
            }
            return Ok(true);
        }
        if self.help_visible {
            if key == Key::Help || key == Key::Escape {
                self.help_visible = false;
            }
            return Ok(true);
        }
        match key {
            Key::Char(ch) => self.document.insert(ch),
            Key::Enter => self.document.insert('\n'),
            Key::Backspace => self.document.backspace(),
            Key::Delete => self.document.delete(),
            Key::Left => self.document.left(),
            Key::Right => self.document.right(),
            Key::Up => self.document.move_visual_rows(page_width, -1),
            Key::Down => self.document.move_visual_rows(page_width, 1),
            Key::Home => self.document.home(),
            Key::End => self.document.end(),
            Key::PageUp => self
                .document
                .move_visual_rows(page_width, -(page_height as isize)),
            Key::PageDown => self
                .document
                .move_visual_rows(page_width, page_height as isize),
            Key::Save => self.save()?,
            Key::New => self.new_note()?,
            Key::Quit => {
                self.save()?;
                return Ok(false);
            }
            Key::Help => self.help_visible = !self.help_visible,
            Key::Browser | Key::Escape => {
                if self.path.starts_with(self.category_directory(2)) {
                    self.secret_unlocked = false;
                }
                self.screen = Screen::Categories;
            }
            Key::Theme => {
                self.theme = (self.theme + 1) % THEMES.len() as u8;
                self.save_display_config();
                self.message = THEMES[self.theme as usize].name.into();
            }
            Key::Rotate => {
                self.rotation = (self.rotation + 1) % 4;
                self.scroll = 0;
                self.save_display_config();
            }
            Key::Larger => {
                self.zoom = (self.zoom + 1).min(5);
                self.scroll = 0;
                self.save_display_config();
            }
            Key::Smaller => {
                self.zoom = (self.zoom - 1).max(-8);
                self.scroll = 0;
                self.save_display_config();
            }
            Key::Redraw | Key::Unknown => {}
        }
        Ok(true)
    }
}

#[cfg(target_os = "linux")]
fn visible_width(text: &str) -> usize {
    text.chars()
        .fold(0, |column, ch| column + char_width(ch, column))
}

fn truncate(text: &str, width: usize) -> String {
    let mut result = String::new();
    let mut column = 0;
    for ch in text.chars() {
        let w = char_width(ch, column);
        if column + w > width {
            break;
        }
        result.push(ch);
        column += w;
    }
    result
}

fn atomic_write(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)?;
    file.write_all(contents)?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    if let Some(parent) = path.parent() {
        if let Ok(directory) = File::open(parent) {
            let _ = directory.sync_all();
        }
    }
    Ok(())
}

fn latest_note(directory: &Path) -> Option<PathBuf> {
    fs::read_dir(directory)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().and_then(|e| e.to_str()) == Some("md"))
        .max_by_key(|entry| {
            entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(UNIX_EPOCH)
        })
        .map(|entry| entry.path())
}

fn notes_for_category(directory: &Path, category_index: usize) -> Vec<PathBuf> {
    let mut notes = Vec::new();
    let mut collect = |folder: &Path| {
        if let Ok(entries) = fs::read_dir(folder) {
            notes.extend(
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .filter(|path| {
                        path.is_file()
                            && path.extension().and_then(|extension| extension.to_str())
                                == Some("md")
                    }),
            );
        }
    };
    if category_index == 0 {
        // Root-level Markdown files are legacy notes. Keep them in place and list them with Notes.
        collect(directory);
    }
    collect(&directory.join(CATEGORIES[category_index.min(CATEGORIES.len() - 1)].1));
    notes.sort_by_key(|path| {
        std::cmp::Reverse(
            fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .unwrap_or(UNIX_EPOCH),
        )
    });
    notes
}

fn new_note_path(directory: &Path) -> PathBuf {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let base = format!("note-{seconds}");
    let mut path = directory.join(format!("{base}.md"));
    let mut suffix = 2;
    while path.exists() {
        path = directory.join(format!("{base}-{suffix}.md"));
        suffix += 1;
    }
    path
}

fn home_directory() -> PathBuf {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .or_else(|| {
            let drive = env::var_os("HOMEDRIVE")?;
            let path = env::var_os("HOMEPATH")?;
            Some(PathBuf::from(drive).join(path))
        })
        .unwrap_or_else(|| PathBuf::from("."))
}

fn config_directory() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        return env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home_directory().join("AppData/Roaming"))
            .join("QuietWrite");
    }
    #[cfg(target_os = "macos")]
    {
        return home_directory().join("Library/Application Support/QuietWrite");
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home_directory().join(".config"))
            .join("quietwrite")
    }
}

fn default_directory() -> PathBuf {
    env::var_os("QUIETWRITE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_directory().join("Writing"))
}

fn default_config_path() -> PathBuf {
    config_directory().join("display.conf")
}

fn default_secret_lock_path() -> PathBuf {
    config_directory().join("secret.lock")
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hex_decode(text: &str) -> io::Result<Vec<u8>> {
    if text.len() % 2 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid lock data",
        ));
    }
    (0..text.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&text[index..index + 2], 16)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid lock data"))
        })
        .collect()
}

fn password_hash(password: &[u8], salt: &[u8]) -> io::Result<[u8; 32]> {
    let params = Params::new(4096, 2, 1, Some(32))
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut output = [0_u8; 32];
    argon2
        .hash_password_into(password, salt, &mut output)
        .map_err(|error| io::Error::other(error.to_string()))?;
    Ok(output)
}

type PasswordBytes = [u8];

fn write_secret_lock(path: &Path, input: &PasswordBytes) -> io::Result<()> {
    let mut salt = [0_u8; 16];
    getrandom::fill(&mut salt).map_err(|error| io::Error::other(error.to_string()))?;
    let hash = password_hash(input, &salt)?;
    atomic_write(
        path,
        format!("v1:{}:{}\n", hex_encode(&salt), hex_encode(&hash)).as_bytes(),
    )?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

fn verify_secret_lock(path: &Path, input: &PasswordBytes) -> io::Result<bool> {
    let contents = fs::read_to_string(path)?;
    let mut fields = contents.trim().split(':');
    if fields.next() != Some("v1") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported lock format",
        ));
    }
    let salt = hex_decode(fields.next().unwrap_or_default())?;
    let expected = hex_decode(fields.next().unwrap_or_default())?;
    if salt.len() != 16 || expected.len() != 32 || fields.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid lock data",
        ));
    }
    let actual = password_hash(input, &salt)?;
    let difference = actual
        .iter()
        .zip(expected.iter())
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        });
    Ok(difference == 0)
}

fn read_display_config(path: &Path) -> (u8, i8, u8) {
    let mut rotation = 0_u8;
    let mut zoom = -2_i8;
    let mut theme = 0_u8;
    if let Ok(contents) = fs::read_to_string(path) {
        for line in contents.lines() {
            if let Some(value) = line.strip_prefix("rotation=") {
                rotation = value.parse::<u8>().unwrap_or(0) % 4;
            } else if let Some(value) = line.strip_prefix("zoom=") {
                zoom = value.parse::<i8>().unwrap_or(0).clamp(-8, 5);
            } else if let Some(value) = line.strip_prefix("theme=") {
                theme = value.parse::<u8>().unwrap_or(0) % THEMES.len() as u8;
            }
        }
    }
    (rotation, zoom, theme)
}

fn tui_color(color: (u8, u8, u8)) -> Color {
    Color::Rgb(color.0, color.1, color.2)
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100_u16.saturating_sub(height)) / 2),
            Constraint::Percentage(height),
            Constraint::Min(0),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100_u16.saturating_sub(width)) / 2),
            Constraint::Percentage(width),
            Constraint::Min(0),
        ])
        .split(vertical[1])[1]
}

fn draw_tui(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let palette = THEMES[app.theme as usize % THEMES.len()];
    let background = Style::default()
        .bg(tui_color(palette.background))
        .fg(tui_color(palette.foreground));
    frame.render_widget(Block::default().style(background), area);

    if app.screen != Screen::Editor {
        draw_tui_browser(frame, app, area, palette);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);
    let editor = chunks[0];
    let content_width = editor.width.saturating_sub(4).max(1) as usize;
    let content_height = editor.height.max(1) as usize;
    let ranges = wrapped_ranges(&app.document.text, content_width);
    let (cursor_row, cursor_column) =
        cursor_position(&app.document.text, &ranges, app.document.cursor);
    if cursor_row < app.scroll {
        app.scroll = cursor_row;
    }
    if cursor_row >= app.scroll + content_height {
        app.scroll = cursor_row + 1 - content_height;
    }
    let lines: Vec<Line> = ranges
        .iter()
        .skip(app.scroll)
        .take(content_height)
        .map(|&(start, end)| {
            let text: String = app.document.text[start..end]
                .iter()
                .filter(|character| !character.is_control() || **character == '\t')
                .collect();
            Line::from(text)
        })
        .collect();
    let prompt = if app.document.text.is_empty() {
        let seed = app
            .path
            .to_string_lossy()
            .bytes()
            .fold(0_usize, |sum, byte| sum.wrapping_add(byte as usize));
        Some(BLANK_PROMPTS[seed % BLANK_PROMPTS.len()])
    } else {
        None
    };
    let editor_block = Block::default()
        .padding(ratatui::widgets::Padding::horizontal(2))
        .style(background);
    let paragraph = if let Some(prompt) = prompt {
        Paragraph::new(prompt)
            .style(
                Style::default()
                    .fg(tui_color(palette.muted))
                    .bg(tui_color(palette.background)),
            )
            .alignment(Alignment::Center)
            .block(editor_block)
    } else {
        Paragraph::new(lines).style(background).block(editor_block)
    };
    frame.render_widget(paragraph, editor);

    let name = app
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("note.md");
    let category = app
        .path
        .parent()
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("Notes");
    let status = format!(
        " {category} / {name}  •  {} words  •  {} chars  •  {} ",
        app.document.word_count(),
        app.document.character_count(),
        if app.document.dirty {
            "writing"
        } else {
            &app.message
        }
    );
    frame.render_widget(
        Paragraph::new(truncate(&status, chunks[1].width as usize)).style(
            Style::default()
                .bg(tui_color(palette.status))
                .fg(tui_color(palette.foreground))
                .add_modifier(Modifier::BOLD),
        ),
        chunks[1],
    );
    frame.render_widget(
        Paragraph::new(" F1 help  ·  F2 browse  ·  F5 theme  ·  Ctrl+S save  ·  Ctrl+Q quit ")
            .style(
                Style::default()
                    .bg(tui_color(palette.background))
                    .fg(tui_color(palette.muted)),
            ),
        chunks[2],
    );

    if !app.help_visible {
        let x = editor
            .x
            .saturating_add(2)
            .saturating_add(cursor_column.min(content_width) as u16)
            .min(editor.right().saturating_sub(1));
        let y = editor
            .y
            .saturating_add(cursor_row.saturating_sub(app.scroll) as u16)
            .min(editor.bottom().saturating_sub(1));
        frame.set_cursor_position((x, y));
    } else {
        let popup = centered_rect(64, 70, area);
        frame.render_widget(Clear, popup);
        let help = Paragraph::new(vec![
            Line::from(Span::styled(
                "QuietWrite keys",
                Style::default()
                    .fg(tui_color(palette.accent))
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("Ctrl+S   Save"),
            Line::from("Ctrl+N   New note"),
            Line::from("Ctrl+Q   Save and quit"),
            Line::from("F2       Browse shelves"),
            Line::from("F5       Change theme"),
            Line::from("F1 / Esc Close help"),
        ])
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Help ")
                .style(background),
        )
        .wrap(Wrap { trim: false });
        frame.render_widget(help, popup);
    }
}

fn draw_tui_browser(frame: &mut Frame, app: &mut App, area: Rect, palette: ThemePalette) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .split(area);
    let title = if app.screen == Screen::Categories {
        "Choose a shelf"
    } else {
        CATEGORIES[app.category_index].0
    };
    frame.render_widget(
        Paragraph::new(title)
            .alignment(Alignment::Center)
            .style(
                Style::default()
                    .fg(tui_color(palette.accent))
                    .bg(tui_color(palette.background))
                    .add_modifier(Modifier::BOLD),
            )
            .block(Block::default().borders(Borders::BOTTOM)),
        chunks[0],
    );
    let entries: Vec<ListItem> = if app.screen == Screen::Categories {
        CATEGORIES
            .iter()
            .map(|(name, _)| ListItem::new(format!("  {name}")))
            .collect()
    } else if app.documents.is_empty() {
        vec![ListItem::new("  No writing yet — Enter creates one")]
    } else {
        app.documents
            .iter()
            .map(|path| {
                let name = path
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .unwrap_or("Untitled");
                ListItem::new(format!("  {name}"))
            })
            .collect()
    };
    let selected = if app.screen == Screen::Categories {
        app.category_index
    } else {
        app.document_index
    };
    let mut state = ListState::default().with_selected(Some(selected));
    let list = List::new(entries)
        .highlight_symbol("› ")
        .highlight_style(
            Style::default()
                .fg(tui_color(palette.foreground))
                .bg(tui_color(palette.status))
                .add_modifier(Modifier::BOLD),
        )
        .style(
            Style::default()
                .fg(tui_color(palette.muted))
                .bg(tui_color(palette.background)),
        )
        .block(Block::default().borders(Borders::LEFT | Borders::RIGHT));
    frame.render_stateful_widget(list, chunks[1], &mut state);
    let footer = if app.screen == Screen::Categories {
        if app.category_index == 2 {
            " ↑↓ choose · Enter unlock · F5 theme · F7/F8 size on Pi · app lock only "
        } else {
            " ↑↓ choose · Enter open · F5 theme · F7/F8 size on Pi · Ctrl+Q quit "
        }
    } else {
        " ↑↓ choose · Enter open · Ctrl+N new · F2/Esc back "
    };
    frame.render_widget(
        Paragraph::new(footer).style(
            Style::default()
                .fg(tui_color(palette.muted))
                .bg(tui_color(palette.background)),
        ),
        chunks[2],
    );
    if let Some(prompt) = app.lock_prompt {
        let popup = centered_rect(60, 35, area);
        frame.render_widget(Clear, popup);
        let title = match prompt {
            LockPrompt::Create => " Create Secret Thoughts password ",
            LockPrompt::Confirm => " Confirm password ",
            LockPrompt::Unlock => " Unlock Secret Thoughts ",
        };
        let bullets = "•".repeat(app.password_input.chars().count());
        let content = vec![
            Line::from(""),
            Line::from(Span::styled(
                bullets,
                Style::default().fg(tui_color(palette.accent)),
            )),
            Line::from(""),
            Line::from(app.message.clone()),
            Line::from(""),
            Line::from("Enter continue · Esc cancel"),
        ];
        frame.render_widget(
            Paragraph::new(content).alignment(Alignment::Center).block(
                Block::default().borders(Borders::ALL).title(title).style(
                    Style::default()
                        .fg(tui_color(palette.foreground))
                        .bg(tui_color(palette.background)),
                ),
            ),
            popup,
        );
    }
}

fn crossterm_key(event: KeyEvent) -> Key {
    if event.kind == KeyEventKind::Release {
        return Key::Unknown;
    }
    if event.modifiers.contains(KeyModifiers::CONTROL) {
        return match event.code {
            KeyCode::Char('q') => Key::Quit,
            KeyCode::Char('s') => Key::Save,
            KeyCode::Char('n') => Key::New,
            KeyCode::Char('o') => Key::Browser,
            KeyCode::Char('l') => Key::Redraw,
            _ => Key::Unknown,
        };
    }
    match event.code {
        KeyCode::Char(character) => Key::Char(character),
        KeyCode::Enter => Key::Enter,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Esc => Key::Escape,
        KeyCode::F(1) => Key::Help,
        KeyCode::F(2) => Key::Browser,
        KeyCode::F(5) => Key::Theme,
        KeyCode::F(6) => Key::Rotate,
        KeyCode::F(7) => Key::Larger,
        KeyCode::F(8) => Key::Smaller,
        KeyCode::Tab => Key::Char('\t'),
        _ => Key::Unknown,
    }
}

struct TuiRestore;

impl Drop for TuiRestore {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
    }
}

fn run_tui(mut app: App) -> Result<(), Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, Hide)?;
    let _restore = TuiRestore;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = RatatuiTerminal::new(backend)?;
    terminal.clear()?;
    loop {
        terminal.draw(|frame| draw_tui(frame, &mut app))?;
        let mut changed = false;
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => {
                    let area = terminal.size()?;
                    let page_height = area.height.saturating_sub(2).max(1) as usize;
                    let page_width = area.width.saturating_sub(4).max(1) as usize;
                    if !app.handle(crossterm_key(key), page_height, page_width)? {
                        break;
                    }
                    changed = true;
                }
                Event::Paste(text) if app.screen == Screen::Editor && !app.help_visible => {
                    for character in text.chars() {
                        app.document.insert(character);
                    }
                    changed = true;
                }
                Event::Resize(_, _) => changed = true,
                _ => {}
            }
        }
        if app.document.dirty && app.last_save.elapsed() >= AUTOSAVE_INTERVAL {
            app.save()?;
            changed = true;
        }
        if !changed {
            continue;
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn run_framebuffer(
    mut app: App,
    mut framebuffer: Framebuffer,
) -> Result<(), Box<dyn std::error::Error>> {
    let terminal = Terminal::enter()?;
    let mut pending = Vec::new();
    let mut input = io::stdin();
    app.render_framebuffer(&mut framebuffer)?;
    loop {
        let mut descriptor = libc::pollfd {
            fd: terminal.fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut descriptor, 1, 100) };
        let mut changed = false;
        if ready > 0 && descriptor.revents & libc::POLLIN != 0 {
            let mut buffer = [0_u8; 512];
            let count = input.read(&mut buffer)?;
            pending.extend_from_slice(&buffer[..count]);
            while let Some(key) = decode_key(&mut pending) {
                let layout = framebuffer.layout(app.rotation, app.zoom);
                if !app.handle(key, layout.content_rows, layout.columns)? {
                    return Ok(());
                }
                changed = true;
            }
        } else if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error.into());
            }
        }
        if app.document.dirty && app.last_save.elapsed() >= AUTOSAVE_INTERVAL {
            app.save()?;
            changed = true;
        }
        if changed {
            app.render_framebuffer(&mut framebuffer)?;
        }
    }
}

fn print_help() {
    println!("{APP_NAME} {}\n\nUsage: quietwrite [--new] [--dir DIRECTORY] [FILE]\n\nNotes default to ~/Writing. Autosave runs every 2 seconds.", env!("CARGO_PKG_VERSION"));
}

fn parse_args() -> Result<(PathBuf, Option<PathBuf>, bool), String> {
    let mut args = env::args_os().skip(1);
    let mut directory = default_directory();
    let mut explicit = None;
    let mut force_new = false;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--help") | Some("-h") => {
                print_help();
                std::process::exit(0);
            }
            Some("--version") | Some("-V") => {
                println!("quietwrite {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            Some("--new") | Some("-n") => force_new = true,
            Some("--dir") => {
                directory = PathBuf::from(args.next().ok_or("--dir requires a directory")?)
            }
            Some(value) if value.starts_with('-') => {
                return Err(format!("unknown option: {value}"))
            }
            _ if explicit.is_none() => {
                let path = PathBuf::from(arg);
                if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                    directory = parent.to_path_buf();
                }
                explicit = Some(path);
            }
            _ => return Err("only one file may be opened".into()),
        }
    }
    Ok((directory, explicit, force_new))
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let (directory, explicit, force_new) =
        parse_args().map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let app = App::open(directory, explicit, force_new)?;
    #[cfg(target_os = "linux")]
    {
        let is_ssh = env::var_os("SSH_CONNECTION").is_some() || env::var_os("SSH_TTY").is_some();
        let framebuffer = if env::var_os("QUIETWRITE_TERMINAL").is_none() && !is_ssh {
            Framebuffer::open().ok()
        } else {
            None
        };
        if let Some(framebuffer) = framebuffer {
            return run_framebuffer(app, framebuffer);
        }
    }
    run_tui(app)
}

fn main() {
    if let Err(error) = run() {
        eprintln!("quietwrite: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_directory(label: &str) -> PathBuf {
        env::temp_dir().join(format!(
            "quietwrite-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn editing_and_movement_work() {
        let mut doc = Document::from_string("one\ntwo".into());
        doc.home();
        assert_eq!(doc.cursor, 4);
        doc.move_visual_rows(80, -1);
        assert_eq!(doc.cursor, 0);
        doc.end();
        assert_eq!(doc.cursor, 3);
        doc.move_visual_rows(80, 1);
        assert_eq!(doc.cursor, 7);
        doc.backspace();
        assert_eq!(doc.as_string(), "one\ntw");
        assert_eq!(doc.word_count(), 2);
        assert_eq!(doc.character_count(), 6);
    }

    #[test]
    fn wrapping_preserves_every_character() {
        let chars: Vec<char> = "abcdef\ngh".chars().collect();
        let ranges = wrapped_ranges(&chars, 3);
        assert_eq!(ranges, vec![(0, 3), (3, 6), (7, 9)]);
        assert_eq!(cursor_position(&chars, &ranges, 3), (1, 0));
    }

    #[test]
    fn arrows_move_across_soft_wrapped_rows() {
        let mut doc = Document::from_string("abcdefgh".into());
        doc.cursor = 6;
        doc.move_visual_rows(4, -1);
        assert_eq!(doc.cursor, 2);
        doc.move_visual_rows(4, 1);
        assert_eq!(doc.cursor, 6);
    }

    #[test]
    fn key_decoder_handles_split_escape_sequence() {
        let mut bytes = vec![27, b'['];
        assert_eq!(decode_key(&mut bytes), None);
        bytes.push(b'A');
        assert_eq!(decode_key(&mut bytes), Some(Key::Up));
        assert!(bytes.is_empty());
    }

    #[test]
    fn function_keys_control_display() {
        let mut bytes = b"\x1b[[A\x1b[15~\x1b[17~\x1b[18~\x1b[19~".to_vec();
        assert_eq!(decode_key(&mut bytes), Some(Key::Help));
        assert_eq!(decode_key(&mut bytes), Some(Key::Theme));
        assert_eq!(decode_key(&mut bytes), Some(Key::Rotate));
        assert_eq!(decode_key(&mut bytes), Some(Key::Larger));
        assert_eq!(decode_key(&mut bytes), Some(Key::Smaller));
        assert!(bytes.is_empty());
    }

    #[test]
    fn atomic_write_replaces_file() {
        let directory = test_directory("atomic");
        let path = directory.join("note.md");
        atomic_write(&path, b"first").unwrap();
        atomic_write(&path, b"second").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "second");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn notes_shelf_includes_legacy_root_files_without_moving_them() {
        let directory = test_directory("legacy");
        fs::create_dir_all(directory.join("Notes")).unwrap();
        fs::create_dir_all(directory.join("Poems")).unwrap();
        atomic_write(&directory.join("legacy.md"), b"still here").unwrap();
        atomic_write(&directory.join("Notes/new.md"), b"new shelf").unwrap();
        atomic_write(&directory.join("Poems/verse.md"), b"poem").unwrap();
        let notes = notes_for_category(&directory, 0);
        assert!(notes.contains(&directory.join("legacy.md")));
        assert!(notes.contains(&directory.join("Notes/new.md")));
        assert!(!notes.contains(&directory.join("Poems/verse.md")));
        assert_eq!(
            fs::read_to_string(directory.join("legacy.md")).unwrap(),
            "still here"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn new_documents_use_the_selected_category() {
        let directory = test_directory("category");
        let mut app = App::open(directory.clone(), None, false).unwrap();
        app.category_index = 2;
        app.new_note().unwrap();
        assert_eq!(
            app.path.parent(),
            Some(directory.join("Secret Thoughts").as_path())
        );
        assert_eq!(app.screen, Screen::Editor);
        assert!(notes_for_category(&directory, 0).is_empty());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn browser_moves_between_shelves_documents_and_editor() {
        let directory = test_directory("browser");
        let mut app = App::open(directory.clone(), None, false).unwrap();
        assert_eq!(app.screen, Screen::Categories);
        app.handle(Key::Enter, 20, 80).unwrap();
        assert_eq!(app.screen, Screen::Documents);
        app.handle(Key::Browser, 20, 80).unwrap();
        assert_eq!(app.screen, Screen::Categories);
        app.handle(Key::Escape, 20, 80).unwrap();
        assert_eq!(app.screen, Screen::Editor);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn secret_lock_verifier_accepts_only_matching_input() {
        let directory = test_directory("lock");
        let path = directory.join("secret.lock");
        write_secret_lock(&path, b"sample phrase").unwrap();
        assert!(verify_secret_lock(&path, b"sample phrase").unwrap());
        assert!(!verify_secret_lock(&path, b"different phrase").unwrap());
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn secret_shelf_prompts_and_locks_again_when_leaving() {
        let directory = test_directory("secret-flow");
        let mut app = App::open(directory.clone(), None, false).unwrap();
        app.secret_lock_path = directory.join("secret.lock");
        app.category_index = 2;
        app.handle(Key::Enter, 20, 80).unwrap();
        assert_eq!(app.lock_prompt, Some(LockPrompt::Create));
        app.password_input = "sample phrase".into();
        app.handle(Key::Enter, 20, 80).unwrap();
        app.password_input = "sample phrase".into();
        app.handle(Key::Enter, 20, 80).unwrap();
        assert_eq!(app.screen, Screen::Documents);
        assert!(app.secret_unlocked);
        app.handle(Key::Escape, 20, 80).unwrap();
        assert!(!app.secret_unlocked);
        fs::remove_dir_all(directory).unwrap();
    }
}
