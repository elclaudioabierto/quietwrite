use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    XChaCha20Poly1305, XNonce,
};
use chrono::Local;
use pulldown_cmark::{html, Options, Parser};
use std::collections::HashSet;
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
use std::sync::{
    atomic::{AtomicU8, Ordering},
    Arc,
};
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
use zeroize::Zeroize;

#[cfg(target_os = "linux")]
mod framebuffer;
#[cfg(target_os = "linux")]
use framebuffer::Framebuffer;

const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(2);
const NETWORK_CHECK_INTERVAL: Duration = Duration::from_secs(30);
#[cfg(not(test))]
const NETWORK_TIMEOUT: Duration = Duration::from_millis(700);
#[cfg(target_os = "linux")]
const KDSETMODE: libc::c_ulong = 0x4B3A;
#[cfg(target_os = "linux")]
const KD_TEXT: libc::c_int = 0;
#[cfg(target_os = "linux")]
const KD_GRAPHICS: libc::c_int = 1;
const APP_NAME: &str = "QuietWrite";

#[derive(Debug, Clone)]
struct Document {
    text: Vec<char>,
    cursor: usize,
    dirty: bool,
    preferred_column: Option<usize>,
    undo: Vec<(Vec<char>, usize)>,
    redo: Vec<(Vec<char>, usize)>,
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
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    fn checkpoint(&mut self) {
        self.undo.push((self.text.clone(), self.cursor));
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn insert(&mut self, ch: char) {
        self.checkpoint();
        self.text.insert(self.cursor, ch);
        self.cursor += 1;
        self.dirty = true;
        self.preferred_column = None;
    }

    fn backspace(&mut self) {
        if self.cursor > 0 {
            self.checkpoint();
            self.cursor -= 1;
            self.text.remove(self.cursor);
            self.dirty = true;
            self.preferred_column = None;
        }
    }

    fn delete(&mut self) {
        if self.cursor < self.text.len() {
            self.checkpoint();
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

    fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        self.redo.push((self.text.clone(), self.cursor));
        (self.text, self.cursor) = previous;
        self.dirty = true;
        self.preferred_column = None;
        true
    }

    fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push((self.text.clone(), self.cursor));
        (self.text, self.cursor) = next;
        self.dirty = true;
        self.preferred_column = None;
        true
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
    Undo,
    Redo,
    Restore,
    Search,
    FindNext,
    Sprint,
    Target,
    Outline,
    PreviousHeading,
    NextHeading,
    SplitView,
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
        26 => Some(Key::Undo),
        25 => Some(Key::Redo),
        18 => Some(Key::Restore),
        6 => Some(Key::Search),
        7 => Some(Key::Target),
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
            (b"\x1b[1;5A", Key::PreviousHeading),
            (b"\x1b[1;5B", Key::NextHeading),
            (b"\x1b[5A", Key::PreviousHeading),
            (b"\x1b[5B", Key::NextHeading),
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
            (b"\x1bOR", Key::FindNext),
            (b"\x1b[13~", Key::FindNext),
            (b"\x1bOS", Key::Sprint),
            (b"\x1b[14~", Key::Sprint),
            (b"\x1b[15~", Key::Theme),
            (b"\x1b[[E", Key::Theme),
            (b"\x1b[17~", Key::Rotate),
            (b"\x1b[18~", Key::Larger),
            (b"\x1b[19~", Key::Smaller),
            (b"\x1b[20~", Key::Outline),
            (b"\x1b[21~", Key::SplitView),
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
                    Key::FindNext => Key::FindNext,
                    Key::Sprint => Key::Sprint,
                    Key::Theme => Key::Theme,
                    Key::Rotate => Key::Rotate,
                    Key::Larger => Key::Larger,
                    Key::Smaller => Key::Smaller,
                    Key::Outline => Key::Outline,
                    Key::SplitView => Key::SplitView,
                    Key::PreviousHeading => Key::PreviousHeading,
                    Key::NextHeading => Key::NextHeading,
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

fn markdown_headings(text: &[char]) -> Vec<(usize, usize, String)> {
    let mut headings = Vec::new();
    let mut start = 0;
    for end in (0..=text.len()).filter(|index| *index == text.len() || text[*index] == '\n') {
        let line = &text[start..end];
        let level = line
            .iter()
            .take_while(|character| **character == '#')
            .count();
        if (1..=6).contains(&level) && line.get(level) == Some(&' ') {
            let title: String = line[level + 1..].iter().collect();
            if !title.trim().is_empty() {
                headings.push((start, level, title));
            }
        }
        start = end.saturating_add(1);
    }
    headings
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
        name: "White on Black",
        background: (0, 0, 0),
        foreground: (255, 255, 255),
        muted: (210, 210, 210),
        accent: (255, 255, 255),
        status: (35, 35, 35),
    },
    ThemePalette {
        name: "Black on White",
        background: (255, 255, 255),
        foreground: (0, 0, 0),
        muted: (55, 55, 55),
        accent: (0, 55, 130),
        status: (225, 225, 225),
    },
    ThemePalette {
        name: "Moon Ink",
        background: (0, 0, 0),
        foreground: (255, 255, 255),
        muted: (218, 224, 232),
        accent: (126, 200, 255),
        status: (20, 43, 68),
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
    ("Drafts", "Drafts"),
    ("Journal", "Journal"),
    ("Poems", "Poems"),
    ("Ideas", "Ideas"),
    ("Projects", "Projects"),
    ("Secret Thoughts", "Secret Thoughts"),
    ("Archive", "Archive"),
    ("Trash", ".quietwrite/Trash"),
];
const PROJECTS_CATEGORY_INDEX: usize = 5;
const SECRET_CATEGORY_INDEX: usize = 6;
const ARCHIVE_CATEGORY_INDEX: usize = 7;
const TRASH_CATEGORY_INDEX: usize = 8;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextPrompt {
    Rename,
    SearchShelf,
    FindDraft,
    WordTarget,
    NewProject,
    NewChapter,
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
    secret_key: Option<[u8; 32]>,
    secret_lock_path: PathBuf,
    text_prompt: Option<TextPrompt>,
    prompt_input: String,
    shelf_search: String,
    find_query: String,
    session_started: Instant,
    session_initial_words: usize,
    sprint_started: Option<Instant>,
    word_target: Option<usize>,
    pinned: HashSet<PathBuf>,
    state_path: PathBuf,
    outline_visible: bool,
    split_visible: bool,
    reference_path: Option<PathBuf>,
    reference_text: Vec<char>,
    network_status: Arc<AtomicU8>,
    last_network_check: Instant,
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
        let state_path = directory.join(".quietwrite/state");
        let screen = if explicit_open || force_new {
            Screen::Editor
        } else {
            Screen::Categories
        };
        let mut document = Document::from_string(text);
        if let Some(cursor) = read_cursor_state(&state_path, &path) {
            document.cursor = cursor.min(document.text.len());
        }
        let session_initial_words = document.word_count();
        let pinned = read_pins(&directory.join(".quietwrite/pins"));
        let network_status = Arc::new(AtomicU8::new(0));
        start_network_probe(Arc::clone(&network_status));
        Ok(Self {
            document,
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
            secret_key: None,
            secret_lock_path,
            text_prompt: None,
            prompt_input: String::new(),
            shelf_search: String::new(),
            find_query: String::new(),
            session_started: Instant::now(),
            session_initial_words,
            sprint_started: None,
            word_target: None,
            pinned,
            state_path,
            outline_visible: false,
            split_visible: false,
            reference_path: None,
            reference_text: Vec::new(),
            network_status,
            last_network_check: Instant::now(),
        })
    }

    fn save(&mut self) -> io::Result<()> {
        if !self.document.dirty {
            self.save_cursor_state()?;
            self.save_count = self.save_count.wrapping_add(1);
            self.message = SAVE_MESSAGES[self.save_count % SAVE_MESSAGES.len()].into();
            self.last_save = Instant::now();
            return Ok(());
        }
        self.snapshot_current()?;
        self.write_document(&self.path, self.document.as_string().as_bytes())?;
        self.save_cursor_state()?;
        self.document.dirty = false;
        self.last_save = Instant::now();
        self.save_count = self.save_count.wrapping_add(1);
        self.message = SAVE_MESSAGES[self.save_count % SAVE_MESSAGES.len()].into();
        Ok(())
    }

    fn new_note(&mut self) -> io::Result<()> {
        if self.category_index == SECRET_CATEGORY_INDEX && self.secret_key.is_none() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Unlock Secret Thoughts first",
            ));
        }
        self.save()?;
        let directory = self.category_directory(self.category_index);
        fs::create_dir_all(&directory)?;
        self.path = new_note_path(&directory);
        self.document = Document::from_string(String::new());
        self.document.dirty = true;
        self.scroll = 0;
        self.message = "New note".into();
        self.screen = Screen::Editor;
        self.session_started = Instant::now();
        self.session_initial_words = 0;
        Ok(())
    }

    fn save_cursor_state(&self) -> io::Result<()> {
        let contents = format!("{}\n{}\n", self.path.display(), self.document.cursor);
        atomic_write(&self.state_path, contents.as_bytes())
    }

    fn refresh_network_if_due(&mut self) {
        if self.last_network_check.elapsed() >= NETWORK_CHECK_INTERVAL {
            self.last_network_check = Instant::now();
            self.network_status.store(0, Ordering::Relaxed);
            start_network_probe(Arc::clone(&self.network_status));
        }
    }

    fn info_bar(&self) -> String {
        let connection = match self.network_status.load(Ordering::Relaxed) {
            2 => "● online",
            1 => "○ offline",
            _ => "… checking",
        };
        format!("{}  ·  {connection}", Local::now().format("%H:%M"))
    }

    fn snapshot_current(&self) -> io::Result<()> {
        let Ok(previous) = fs::read(&self.path) else {
            return Ok(());
        };
        if self.read_document_bytes(&self.path)? == self.document.as_string().as_bytes() {
            return Ok(());
        }
        let folder = self
            .directory
            .join(".quietwrite/history")
            .join(history_key(&self.directory, &self.path));
        fs::create_dir_all(&folder)?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        // Secret history remains ciphertext; ordinary history remains Markdown.
        atomic_write(&folder.join(format!("{stamp}.md")), &previous)?;
        let mut versions: Vec<PathBuf> = fs::read_dir(&folder)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect();
        versions.sort();
        let remove_count = versions.len().saturating_sub(20);
        for old in versions.into_iter().take(remove_count) {
            fs::remove_file(old)?;
        }
        Ok(())
    }

    fn restore_snapshot(&mut self) -> io::Result<()> {
        let folder = self
            .directory
            .join(".quietwrite/history")
            .join(history_key(&self.directory, &self.path));
        let latest = fs::read_dir(folder).ok().and_then(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .max()
        });
        if let Some(path) = latest {
            self.document.checkpoint();
            let restored = if self.is_secret_path(&self.path) {
                decrypt_secret(
                    self.secret_key.as_ref().ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            "Secret Thoughts are locked",
                        )
                    })?,
                    &fs::read(path)?,
                )?
            } else {
                fs::read(path)?
            };
            self.document.text = String::from_utf8(restored)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid document text"))?
                .chars()
                .collect();
            self.document.cursor = self.document.text.len();
            self.document.dirty = true;
            self.message = "restored previous version".into();
        } else {
            self.message = "no previous version".into();
        }
        Ok(())
    }

    fn category_directory(&self, index: usize) -> PathBuf {
        self.directory
            .join(CATEGORIES[index.min(CATEGORIES.len() - 1)].1)
    }

    fn is_secret_path(&self, path: &Path) -> bool {
        path.starts_with(self.category_directory(SECRET_CATEGORY_INDEX))
    }

    fn read_document_bytes(&self, path: &Path) -> io::Result<Vec<u8>> {
        let bytes = fs::read(path)?;
        if self.is_secret_path(path) {
            decrypt_secret(
                self.secret_key.as_ref().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "Secret Thoughts are locked",
                    )
                })?,
                &bytes,
            )
        } else {
            Ok(bytes)
        }
    }

    fn read_document(&self, path: &Path) -> io::Result<String> {
        String::from_utf8(self.read_document_bytes(path)?)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid document text"))
    }

    fn write_document(&self, path: &Path, plaintext: &[u8]) -> io::Result<()> {
        if self.is_secret_path(path) {
            let key = self.secret_key.as_ref().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Secret Thoughts are locked",
                )
            })?;
            atomic_write(path, &encrypt_secret(key, plaintext)?)
        } else {
            atomic_write(path, plaintext)
        }
    }

    fn migrate_secret_files(&self) -> io::Result<()> {
        for path in notes_for_category(&self.directory, SECRET_CATEGORY_INDEX) {
            let bytes = fs::read(&path)?;
            if !bytes.starts_with(SECRET_MAGIC) {
                self.write_document(&path, &bytes)?;
            }
        }
        Ok(())
    }

    fn refresh_documents(&mut self) {
        self.documents = notes_for_category(&self.directory, self.category_index);
        if !self.shelf_search.is_empty() {
            let query = self.shelf_search.to_lowercase();
            let paths = std::mem::take(&mut self.documents);
            self.documents = paths
                .into_iter()
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.to_lowercase().contains(&query))
                        || self
                            .read_document(path)
                            .is_ok_and(|text| text.to_lowercase().contains(&query))
                })
                .collect();
        }
        self.documents
            .sort_by_key(|path| !self.pinned.contains(path));
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

    fn lock_secrets(&mut self) {
        self.secret_unlocked = false;
        if let Some(mut key) = self.secret_key.take() {
            key.zeroize();
        }
    }

    fn close_secret_editor(&mut self) -> io::Result<()> {
        self.save()?;
        self.document.text.fill('\0');
        self.document = Document::from_string(String::new());
        self.path = new_note_path(&self.category_directory(0));
        self.scroll = 0;
        self.lock_secrets();
        Ok(())
    }

    fn open_category(&mut self) {
        self.document_index = 0;
        self.refresh_documents();
        self.screen = Screen::Documents;
    }

    fn submit_lock_prompt(&mut self) {
        match self.lock_prompt {
            Some(LockPrompt::Create) => {
                if self.password_input.len() < 8 {
                    self.message = "Use at least 8 characters".into();
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
                match create_secret_lock(&self.secret_lock_path, self.password_input.as_bytes()) {
                    Ok(key) => {
                        self.secret_unlocked = true;
                        self.secret_key = Some(key);
                        if let Err(error) = self.migrate_secret_files() {
                            self.message = format!("Could not encrypt existing notes: {error}");
                            return;
                        }
                        self.lock_prompt = None;
                        self.password_input.clear();
                        self.password_confirmation.clear();
                        self.open_category();
                    }
                    Err(error) => self.message = format!("Could not save lock: {error}"),
                }
            }
            Some(LockPrompt::Unlock) => {
                match unlock_secret(&self.secret_lock_path, self.password_input.as_bytes()) {
                    Ok(Some(key)) => {
                        self.secret_unlocked = true;
                        self.secret_key = Some(key);
                        if let Err(error) = self.migrate_secret_files() {
                            self.message = format!("Could not encrypt existing notes: {error}");
                            return;
                        }
                        self.lock_prompt = None;
                        self.password_input.clear();
                        self.open_category();
                    }
                    Ok(None) => {
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
        let text = self.read_document(&path)?;
        self.document = Document::from_string(text);
        if let Some(cursor) = read_cursor_state(&self.state_path, &path) {
            self.document.cursor = cursor.min(self.document.text.len());
        }
        self.path = path;
        self.scroll = 0;
        self.message = "opened".into();
        self.screen = Screen::Editor;
        self.session_started = Instant::now();
        self.session_initial_words = self.document.word_count();
        Ok(())
    }

    fn selected_path(&self) -> Option<PathBuf> {
        self.documents.get(self.document_index).cloned()
    }

    fn start_prompt(&mut self, prompt: TextPrompt) {
        self.prompt_input.clear();
        self.text_prompt = Some(prompt);
    }

    fn submit_text_prompt(&mut self) -> io::Result<()> {
        let Some(prompt) = self.text_prompt.take() else {
            return Ok(());
        };
        let input = std::mem::take(&mut self.prompt_input);
        match prompt {
            TextPrompt::Rename => {
                if let Some(path) = self.selected_path() {
                    let name = safe_filename(&input);
                    if !name.is_empty() {
                        let destination = path.with_file_name(format!("{name}.md"));
                        fs::rename(&path, &destination)?;
                        if self.pinned.remove(&path) {
                            self.pinned.insert(destination);
                        }
                        self.save_pins()?;
                        self.refresh_documents();
                        self.message = "renamed".into();
                    }
                }
            }
            TextPrompt::SearchShelf => {
                self.shelf_search = input;
                self.refresh_documents();
                self.message = if self.shelf_search.is_empty() {
                    "search cleared".into()
                } else {
                    format!("search: {}", self.shelf_search)
                };
            }
            TextPrompt::FindDraft => {
                self.find_query = input;
                self.find_next();
            }
            TextPrompt::WordTarget => {
                self.word_target = input.parse::<usize>().ok().filter(|target| *target > 0);
                self.message = self.word_target.map_or_else(
                    || "target cleared".into(),
                    |target| format!("target: {target} words"),
                );
            }
            TextPrompt::NewProject => {
                let name = safe_filename(&input);
                if !name.is_empty() {
                    let folder = self.category_directory(PROJECTS_CATEGORY_INDEX).join(&name);
                    fs::create_dir_all(&folder)?;
                    atomic_write(
                        &folder.join("00-title-page.md"),
                        format!("# {name}\n\n**Author Name**\n").as_bytes(),
                    )?;
                    atomic_write(
                        &folder.join("01-copyright.md"),
                        "# Copyright\n\nCopyright © YEAR Author Name\n\nAll rights reserved.\n\nISBN: \n".as_bytes(),
                    )?;
                    let path = folder.join("10-chapter-1.md");
                    atomic_write(&path, b"# Chapter 1\n\n")?;
                    atomic_write(
                        &folder.join("90-about-the-author.md"),
                        b"# About the Author\n\n",
                    )?;
                    self.open_path(path)?;
                    self.message = format!("book: {name}");
                }
            }
            TextPrompt::NewChapter => {
                let name = safe_filename(&input);
                if !name.is_empty() {
                    let projects = self.category_directory(PROJECTS_CATEGORY_INDEX);
                    let folder = self
                        .selected_path()
                        .and_then(|path| path.parent().map(Path::to_path_buf))
                        .filter(|path| path.starts_with(&projects))
                        .unwrap_or(projects);
                    fs::create_dir_all(&folder)?;
                    let filename = format!("{name}.md");
                    let path = unique_destination(&folder, std::ffi::OsStr::new(&filename));
                    atomic_write(&path, format!("# {name}\n\n").as_bytes())?;
                    self.open_path(path)?;
                    self.message = format!("chapter: {name}");
                }
            }
        }
        Ok(())
    }

    fn open_path(&mut self, path: PathBuf) -> io::Result<()> {
        self.save()?;
        let text = self.read_document(&path)?;
        self.document = Document::from_string(text);
        self.path = path;
        self.scroll = 0;
        self.screen = Screen::Editor;
        self.session_started = Instant::now();
        self.session_initial_words = self.document.word_count();
        Ok(())
    }

    fn headings(&self) -> Vec<(usize, usize, String)> {
        markdown_headings(&self.document.text)
    }

    fn move_heading(&mut self, forward: bool) {
        let headings = self.headings();
        let destination = if forward {
            headings
                .iter()
                .find(|(index, _, _)| *index > self.document.cursor)
                .or_else(|| headings.first())
        } else {
            headings
                .iter()
                .rev()
                .find(|(index, _, _)| *index < self.document.cursor)
                .or_else(|| headings.last())
        };
        if let Some((index, _, title)) = destination {
            self.document.cursor = *index;
            self.scroll = 0;
            self.message = format!("heading: {title}");
        } else {
            self.message = "no headings".into();
        }
    }

    fn select_reference(&mut self) -> io::Result<()> {
        if let Some(path) = self.selected_path() {
            self.reference_text = self.read_document(&path)?.chars().collect();
            self.reference_path = Some(path);
            self.split_visible = true;
            self.screen = Screen::Editor;
            self.message = "reference opened".into();
        }
        Ok(())
    }

    fn selected_project_root(&self) -> Option<PathBuf> {
        let projects = self.category_directory(PROJECTS_CATEGORY_INDEX);
        let selected = self.selected_path()?;
        let relative = selected.strip_prefix(&projects).ok()?;
        let project_name = relative.components().next()?;
        Some(projects.join(project_name))
    }

    fn export_selected_book(&mut self) -> io::Result<()> {
        let Some(project) = self.selected_project_root() else {
            self.message = "Select a chapter inside a book project".into();
            return Ok(());
        };
        let title = project
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Untitled Book");
        let mut chapters = Vec::new();
        collect_markdown(&project, true, &mut chapters);
        chapters.sort();
        if chapters.is_empty() {
            self.message = "This book has no chapters".into();
            return Ok(());
        }

        let mut markdown = String::new();
        for (index, chapter) in chapters.iter().enumerate() {
            if index > 0 {
                markdown.push_str("\n\n---\n\n");
            }
            markdown.push_str(&fs::read_to_string(chapter)?);
            if !markdown.ends_with('\n') {
                markdown.push('\n');
            }
        }

        let parser = Parser::new_ext(&markdown, Options::all());
        let mut body = String::new();
        html::push_html(&mut body, parser);
        let escaped_title = html_escape(title);
        let document = format!(
            "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{escaped_title}</title><style>body{{font-family:serif;line-height:1.5;max-width:42em;margin:auto}}h1{{page-break-before:always;text-align:center;margin-top:20%}}h1:first-child{{page-break-before:avoid}}hr{{page-break-after:always;border:0}}p{{orphans:2;widows:2}}</style></head><body>\n{body}\n</body></html>\n"
        );
        let export_folder = self.directory.join("Exports").join(safe_filename(title));
        fs::create_dir_all(&export_folder)?;
        atomic_write(&export_folder.join("manuscript.md"), markdown.as_bytes())?;
        atomic_write(&export_folder.join("manuscript.html"), document.as_bytes())?;
        atomic_write(
            &export_folder.join("KDP-README.txt"),
            b"manuscript.html: upload to KDP as a reflowable eBook or no-bleed paperback manuscript.\nmanuscript.md: portable combined source and conversion input.\nAlways inspect the result in Kindle Previewer or KDP Print Previewer before publishing.\n",
        )?;
        self.message = format!("Exported {title} to Exports");
        Ok(())
    }

    fn find_next(&mut self) {
        if self.find_query.is_empty() {
            self.message = "no find text".into();
            return;
        }
        let text = self.document.as_string();
        let query = self.find_query.to_lowercase();
        let lower = text.to_lowercase();
        let cursor_byte = text
            .char_indices()
            .nth(self.document.cursor)
            .map_or(text.len(), |(index, _)| index);
        let found = lower[cursor_byte..]
            .find(&query)
            .map(|index| cursor_byte + index)
            .or_else(|| lower[..cursor_byte].find(&query));
        if let Some(byte) = found {
            self.document.cursor = text[..byte].chars().count();
            self.message = format!("found: {}", self.find_query);
        } else {
            self.message = format!("not found: {}", self.find_query);
        }
    }

    fn save_pins(&self) -> io::Result<()> {
        let mut paths: Vec<_> = self
            .pinned
            .iter()
            .map(|path| path.display().to_string())
            .collect();
        paths.sort();
        atomic_write(
            &self.directory.join(".quietwrite/pins"),
            paths.join("\n").as_bytes(),
        )
    }

    fn toggle_pin(&mut self) -> io::Result<()> {
        if let Some(path) = self.selected_path() {
            if !self.pinned.remove(&path) {
                self.pinned.insert(path);
                self.message = "pinned".into();
            } else {
                self.message = "unpinned".into();
            }
            self.save_pins()?;
            self.refresh_documents();
        }
        Ok(())
    }

    fn move_selected_to(&mut self, category: usize) -> io::Result<()> {
        if let Some(path) = self.selected_path() {
            let folder = self.category_directory(category);
            fs::create_dir_all(&folder)?;
            let destination = unique_destination(&folder, path.file_name().unwrap_or_default());
            if self.is_secret_path(&path) && !self.is_secret_path(&destination) {
                self.message = "Secret Thoughts cannot leave the encrypted shelf".into();
                return Ok(());
            }
            let crosses_secret_boundary =
                self.is_secret_path(&path) != self.is_secret_path(&destination);
            if self.is_secret_path(&destination) && self.secret_key.is_none() {
                self.message = "Unlock Secret Thoughts before moving a note there".into();
                return Ok(());
            }
            if crosses_secret_boundary {
                let plaintext = self.read_document_bytes(&path)?;
                self.write_document(&destination, &plaintext)?;
                fs::remove_file(&path)?;
            } else {
                fs::rename(&path, &destination)?;
            }
            if self.pinned.remove(&path) {
                self.pinned.insert(destination.clone());
                self.save_pins()?;
            }
            self.category_index = category;
            self.shelf_search.clear();
            self.refresh_documents();
            self.document_index = self
                .documents
                .iter()
                .position(|candidate| candidate == &destination)
                .unwrap_or(0);
            self.message = format!("Moved safely to {}", CATEGORIES[category].0);
        }
        Ok(())
    }

    fn trash_selected(&mut self) -> io::Result<()> {
        if let Some(path) = self.selected_path() {
            if self.is_secret_path(&path) {
                self.message = "Encrypted trash is not available yet".into();
                return Ok(());
            }
            let folder = self.category_directory(TRASH_CATEGORY_INDEX);
            fs::create_dir_all(&folder)?;
            let destination = unique_destination(&folder, path.file_name().unwrap_or_default());
            fs::rename(&path, destination)?;
            self.pinned.remove(&path);
            self.save_pins()?;
            self.refresh_documents();
            self.message = "moved to trash".into();
        }
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
        if let Some(prompt) = self.text_prompt {
            let layout = framebuffer.layout(self.rotation, -3);
            framebuffer.clear(palette.background, self.rotation);
            let heading = match prompt {
                TextPrompt::Rename => "Rename document",
                TextPrompt::SearchShelf => "Search this shelf",
                TextPrompt::FindDraft => "Find in draft",
                TextPrompt::WordTarget => "Session word target",
                TextPrompt::NewProject => "New project",
                TextPrompt::NewChapter => "New chapter",
            };
            framebuffer.text(
                layout.margin_x,
                layout.margin_y + layout.line_height * 2,
                heading,
                layout,
                palette.accent,
                palette.background,
                self.rotation,
            );
            framebuffer.text(
                layout.margin_x,
                layout.margin_y + layout.line_height * 5,
                &format!("{}_", self.prompt_input),
                layout,
                palette.foreground,
                palette.background,
                self.rotation,
            );
            framebuffer.text(
                layout.margin_x,
                layout.logical_height.saturating_sub(layout.line_height * 2),
                "Enter confirm · F2 cancel",
                layout,
                palette.muted,
                palette.background,
                self.rotation,
            );
            framebuffer.flush();
            return Ok(());
        }
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
                        palette.foreground,
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
                        palette.background
                    } else {
                        palette.muted
                    },
                    if index == selected {
                        palette.foreground
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
            let footer = two_sided_line(footer, &self.info_bar(), layout.columns);
            framebuffer.text(
                layout.margin_x,
                layout.logical_height.saturating_sub(layout.line_height * 2),
                &footer,
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
        let help = two_sided_line(help_text, &self.info_bar(), layout.columns);
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
        if self.text_prompt.is_some() {
            match key {
                Key::Char(character) if !character.is_control() => {
                    self.prompt_input.push(character)
                }
                Key::Backspace => {
                    self.prompt_input.pop();
                }
                Key::Enter => self.submit_text_prompt()?,
                Key::Escape | Key::Browser => {
                    self.text_prompt = None;
                    self.prompt_input.clear();
                    self.message = "cancelled".into();
                }
                _ => {}
            }
            return Ok(true);
        }
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
                    if self.category_index == SECRET_CATEGORY_INDEX && !self.secret_unlocked {
                        self.begin_secret_unlock();
                    } else {
                        self.open_category();
                    }
                }
                Key::Enter if self.screen == Screen::Documents => self.open_selected_document()?,
                Key::New if self.screen == Screen::Documents => self.new_note()?,
                Key::Search if self.screen == Screen::Documents => {
                    self.start_prompt(TextPrompt::SearchShelf)
                }
                Key::Char('/') if self.screen == Screen::Documents => {
                    self.start_prompt(TextPrompt::SearchShelf)
                }
                Key::Char('r') if self.screen == Screen::Documents => {
                    self.start_prompt(TextPrompt::Rename)
                }
                Key::Char('p') if self.screen == Screen::Documents => self.toggle_pin()?,
                Key::Char('d') if self.screen == Screen::Documents => self.trash_selected()?,
                Key::Char('a') if self.screen == Screen::Documents => {
                    self.move_selected_to(ARCHIVE_CATEGORY_INDEX)?
                }
                Key::Char('v') if self.screen == Screen::Documents => self.select_reference()?,
                Key::Char('j')
                    if self.screen == Screen::Documents
                        && self.category_index == PROJECTS_CATEGORY_INDEX =>
                {
                    self.start_prompt(TextPrompt::NewProject)
                }
                Key::Char('c')
                    if self.screen == Screen::Documents
                        && self.category_index == PROJECTS_CATEGORY_INDEX =>
                {
                    self.start_prompt(TextPrompt::NewChapter)
                }
                Key::Char('e')
                    if self.screen == Screen::Documents
                        && self.category_index == PROJECTS_CATEGORY_INDEX =>
                {
                    self.export_selected_book()?
                }
                Key::Char('m') if self.screen == Screen::Documents => {
                    let destination = (self.category_index + 1) % CATEGORIES.len();
                    self.move_selected_to(destination)?;
                }
                Key::Browser | Key::Escape if self.screen == Screen::Documents => {
                    if self.category_index == SECRET_CATEGORY_INDEX {
                        self.lock_secrets();
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
            Key::Undo => {
                self.message = if self.document.undo() {
                    "undo"
                } else {
                    "nothing to undo"
                }
                .into();
            }
            Key::Redo => {
                self.message = if self.document.redo() {
                    "redo"
                } else {
                    "nothing to redo"
                }
                .into();
            }
            Key::Restore => self.restore_snapshot()?,
            Key::Search => self.start_prompt(TextPrompt::FindDraft),
            Key::FindNext => self.find_next(),
            Key::Sprint => {
                if self.sprint_started.take().is_some() {
                    self.message = "sprint stopped".into();
                } else {
                    self.sprint_started = Some(Instant::now());
                    self.session_initial_words = self.document.word_count();
                    self.message = "25 minute sprint".into();
                }
            }
            Key::Target => self.start_prompt(TextPrompt::WordTarget),
            Key::Outline => self.outline_visible = !self.outline_visible,
            Key::PreviousHeading => self.move_heading(false),
            Key::NextHeading => self.move_heading(true),
            Key::SplitView => {
                if self.reference_path.is_some() {
                    self.split_visible = !self.split_visible;
                } else {
                    self.message = "choose a reference with v in the browser".into();
                }
            }
            Key::New => self.new_note()?,
            Key::Quit => {
                self.save()?;
                return Ok(false);
            }
            Key::Help => self.help_visible = !self.help_visible,
            Key::Browser | Key::Escape => {
                if self
                    .path
                    .starts_with(self.category_directory(SECRET_CATEGORY_INDEX))
                {
                    self.close_secret_editor()?;
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

#[cfg(any(target_os = "linux", test))]
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

#[cfg(any(target_os = "linux", test))]
fn two_sided_line(left: &str, right: &str, width: usize) -> String {
    let right = truncate(right, width);
    let right_width = right.chars().fold(0, |column, character| {
        column + char_width(character, column)
    });
    if right_width >= width {
        return right;
    }
    let left = truncate(left, width.saturating_sub(right_width + 1));
    let left_width = left.chars().fold(0, |column, character| {
        column + char_width(character, column)
    });
    format!(
        "{left}{}{right}",
        " ".repeat(width.saturating_sub(left_width + right_width))
    )
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

fn start_network_probe(status: Arc<AtomicU8>) {
    #[cfg(test)]
    {
        status.store(1, Ordering::Relaxed);
    }
    #[cfg(not(test))]
    std::thread::spawn(move || {
        let online = ["1.1.1.1:443", "8.8.8.8:443"].iter().any(|address| {
            address
                .parse::<std::net::SocketAddr>()
                .is_ok_and(|address| {
                    std::net::TcpStream::connect_timeout(&address, NETWORK_TIMEOUT).is_ok()
                })
        });
        status.store(if online { 2 } else { 1 }, Ordering::Relaxed);
    });
}

fn safe_filename(input: &str) -> String {
    input
        .trim()
        .trim_end_matches(".md")
        .chars()
        .filter(|character| !matches!(character, '/' | '\\' | ':' | '\0'))
        .collect::<String>()
        .trim()
        .to_string()
}

fn html_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn history_key(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .chars()
        .map(|character| {
            if matches!(character, '/' | '\\' | ':') {
                '_'
            } else {
                character
            }
        })
        .collect()
}

fn unique_destination(folder: &Path, name: &std::ffi::OsStr) -> PathBuf {
    let candidate = folder.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("note");
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("md");
    for suffix in 2.. {
        let candidate = folder.join(format!("{stem}-{suffix}.{extension}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
}

fn read_cursor_state(state_path: &Path, path: &Path) -> Option<usize> {
    let contents = fs::read_to_string(state_path).ok()?;
    let mut lines = contents.lines();
    if Path::new(lines.next()?) != path {
        return None;
    }
    lines.next()?.parse().ok()
}

fn read_pins(path: &Path) -> HashSet<PathBuf> {
    fs::read_to_string(path).map_or_else(
        |_| HashSet::new(),
        |contents| {
            contents
                .lines()
                .filter(|line| !line.is_empty())
                .map(PathBuf::from)
                .collect()
        },
    )
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
    if category_index == 0 {
        // Root-level Markdown files are legacy notes. Keep them in place and list them with Notes.
        collect_markdown(directory, false, &mut notes);
    }
    collect_markdown(
        &directory.join(CATEGORIES[category_index.min(CATEGORIES.len() - 1)].1),
        category_index == PROJECTS_CATEGORY_INDEX,
        &mut notes,
    );
    notes.sort_by_key(|path| {
        std::cmp::Reverse(
            fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .unwrap_or(UNIX_EPOCH),
        )
    });
    notes
}

fn collect_markdown(folder: &Path, recursive: bool, notes: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    for path in entries.filter_map(Result::ok).map(|entry| entry.path()) {
        if recursive && path.is_dir() {
            collect_markdown(&path, true, notes);
        } else if path.is_file()
            && path.extension().and_then(|extension| extension.to_str()) == Some("md")
        {
            notes.push(path);
        }
    }
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
        home_directory().join("Library/Application Support/QuietWrite")
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
    if !text.len().is_multiple_of(2) {
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

const SECRET_MAGIC: &[u8] = b"QWSECRET1\0";

fn password_material(password: &[u8], salt: &[u8]) -> io::Result<[u8; 64]> {
    let params = Params::new(19456, 2, 1, Some(64))
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut output = [0_u8; 64];
    argon2
        .hash_password_into(password, salt, &mut output)
        .map_err(|error| io::Error::other(error.to_string()))?;
    Ok(output)
}

fn legacy_password_hash(password: &[u8], salt: &[u8]) -> io::Result<[u8; 32]> {
    let params = Params::new(4096, 2, 1, Some(32))
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
    let mut output = [0_u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password, salt, &mut output)
        .map_err(|error| io::Error::other(error.to_string()))?;
    Ok(output)
}

type PasswordBytes = [u8];

fn create_secret_lock(path: &Path, input: &PasswordBytes) -> io::Result<[u8; 32]> {
    let mut salt = [0_u8; 16];
    getrandom::fill(&mut salt).map_err(|error| io::Error::other(error.to_string()))?;
    let material = password_material(input, &salt)?;
    let mut key = [0_u8; 32];
    key.copy_from_slice(&material[..32]);
    atomic_write(
        path,
        format!("v2:{}:{}\n", hex_encode(&salt), hex_encode(&material[32..])).as_bytes(),
    )?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(key)
}

fn unlock_secret(path: &Path, input: &PasswordBytes) -> io::Result<Option<[u8; 32]>> {
    let contents = fs::read_to_string(path)?;
    let mut fields = contents.trim().split(':');
    let version = fields.next();
    if version != Some("v2") && version != Some("v1") {
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
    if version == Some("v1") {
        let actual = legacy_password_hash(input, &salt)?;
        let difference = actual
            .iter()
            .zip(expected.iter())
            .fold(0_u8, |d, (a, b)| d | (a ^ b));
        return if difference == 0 {
            create_secret_lock(path, input).map(Some)
        } else {
            Ok(None)
        };
    }
    let material = password_material(input, &salt)?;
    let difference = material[32..]
        .iter()
        .zip(expected.iter())
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        });
    if difference != 0 {
        return Ok(None);
    }
    let mut key = [0_u8; 32];
    key.copy_from_slice(&material[..32]);
    Ok(Some(key))
}

fn encrypt_secret(key: &[u8; 32], plaintext: &[u8]) -> io::Result<Vec<u8>> {
    let mut nonce = [0_u8; 24];
    getrandom::fill(&mut nonce).map_err(|error| io::Error::other(error.to_string()))?;
    let cipher = XChaCha20Poly1305::new(key.into());
    let encrypted = cipher
        .encrypt(XNonce::from_slice(&nonce), plaintext)
        .map_err(|_| io::Error::other("encryption failed"))?;
    let mut output = Vec::with_capacity(SECRET_MAGIC.len() + nonce.len() + encrypted.len());
    output.extend_from_slice(SECRET_MAGIC);
    output.extend_from_slice(&nonce);
    output.extend_from_slice(&encrypted);
    Ok(output)
}

fn decrypt_secret(key: &[u8; 32], ciphertext: &[u8]) -> io::Result<Vec<u8>> {
    if !ciphertext.starts_with(SECRET_MAGIC) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unencrypted Secret Thoughts file",
        ));
    }
    let rest = &ciphertext[SECRET_MAGIC.len()..];
    if rest.len() < 24 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "truncated encrypted file",
        ));
    }
    let (nonce, encrypted) = rest.split_at(24);
    XChaCha20Poly1305::new(key.into())
        .decrypt(XNonce::from_slice(nonce), encrypted)
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "wrong password or damaged encrypted file",
            )
        })
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
        draw_text_prompt(frame, app, area, palette);
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
    let workspace = chunks[0];
    let (outline_area, editor, reference_area) = match (app.outline_visible, app.split_visible) {
        (true, true) => {
            let panes = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(24),
                    Constraint::Percentage(38),
                    Constraint::Percentage(38),
                ])
                .split(workspace);
            (Some(panes[0]), panes[1], Some(panes[2]))
        }
        (true, false) => {
            let panes = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(28), Constraint::Percentage(72)])
                .split(workspace);
            (Some(panes[0]), panes[1], None)
        }
        (false, true) => {
            let panes = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(workspace);
            (None, panes[0], Some(panes[1]))
        }
        (false, false) => (None, workspace, None),
    };
    if let Some(area) = outline_area {
        let headings = app.headings();
        let active = headings
            .iter()
            .rposition(|(index, _, _)| *index <= app.document.cursor);
        let lines: Vec<Line> = headings
            .iter()
            .enumerate()
            .map(|(position, (_, level, title))| {
                let marker = if Some(position) == active { "›" } else { " " };
                Line::from(format!(
                    "{marker} {}{}",
                    "  ".repeat(level.saturating_sub(1)),
                    title
                ))
            })
            .collect();
        frame.render_widget(
            Paragraph::new(lines)
                .style(background)
                .block(Block::default().borders(Borders::RIGHT).title(" Outline "))
                .wrap(Wrap { trim: false }),
            area,
        );
    }
    if let Some(area) = reference_area {
        let width = area.width.saturating_sub(2).max(1) as usize;
        let lines: Vec<Line> = wrapped_ranges(&app.reference_text, width)
            .into_iter()
            .take(area.height.saturating_sub(2) as usize)
            .map(|(start, end)| {
                Line::from(app.reference_text[start..end].iter().collect::<String>())
            })
            .collect();
        let title = app
            .reference_path
            .as_ref()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .unwrap_or("Reference");
        frame.render_widget(
            Paragraph::new(lines)
                .style(
                    Style::default()
                        .fg(tui_color(palette.muted))
                        .bg(tui_color(palette.background)),
                )
                .block(
                    Block::default()
                        .borders(Borders::LEFT)
                        .title(format!(" Reference: {title} ")),
                )
                .wrap(Wrap { trim: false }),
            area,
        );
    }
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
    let session_words = app
        .document
        .word_count()
        .saturating_sub(app.session_initial_words);
    let elapsed = app.session_started.elapsed().as_secs() / 60;
    let target = app
        .word_target
        .map_or_else(String::new, |target| format!(" • target {target}"));
    let sprint = app.sprint_started.map_or_else(String::new, |started| {
        let remaining = 25_u64.saturating_sub(started.elapsed().as_secs() / 60);
        format!(" • sprint {remaining}m")
    });
    let status = format!(
        " {category} / {name}  •  {} words / {} chars  •  +{session_words} in {elapsed}m{target}{sprint}  •  {} ",
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
    draw_info_bar(
        frame,
        app,
        " F1 help · F2 browse · F9 outline · F10 split · Ctrl+↑/↓ headings · Ctrl+S save ",
        chunks[2],
        palette,
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
            Line::from("Ctrl+Z/Y Undo / redo"),
            Line::from("Ctrl+R   Restore previous version"),
            Line::from("Ctrl+F   Find in draft · F3 next"),
            Line::from("F4       Start / stop 25m sprint"),
            Line::from("Ctrl+G   Set word target"),
            Line::from("Ctrl+Q   Save and quit"),
            Line::from("F2       Browse shelves"),
            Line::from("F5       Change theme"),
            Line::from("F9       Toggle outline"),
            Line::from("F10      Toggle reference split"),
            Line::from("Ctrl+↑/↓ Previous / next heading"),
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
    draw_text_prompt(frame, app, area, palette);
}

fn draw_text_prompt(frame: &mut Frame, app: &App, area: Rect, palette: ThemePalette) {
    let Some(prompt) = app.text_prompt else {
        return;
    };
    let title = match prompt {
        TextPrompt::Rename => " Rename document ",
        TextPrompt::SearchShelf => " Search this shelf ",
        TextPrompt::FindDraft => " Find in draft ",
        TextPrompt::WordTarget => " Session word target ",
        TextPrompt::NewProject => " New project ",
        TextPrompt::NewChapter => " New chapter ",
    };
    let popup = centered_rect(64, 22, area);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(format!(
            "\n  {}_\n\n  Enter confirm · Esc cancel",
            app.prompt_input
        ))
        .style(
            Style::default()
                .fg(tui_color(palette.foreground))
                .bg(tui_color(palette.background)),
        )
        .block(Block::default().borders(Borders::ALL).title(title)),
        popup,
    );
}

fn draw_info_bar(frame: &mut Frame, app: &App, left: &str, area: Rect, palette: ThemePalette) {
    let info = format!(" {} ", app.info_bar());
    let width = info.chars().count().min(u16::MAX as usize) as u16;
    let sections = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(1), Constraint::Length(width)])
        .split(area);
    let style = Style::default()
        .bg(tui_color(palette.background))
        .fg(tui_color(palette.muted));
    frame.render_widget(Paragraph::new(left).style(style), sections[0]);
    frame.render_widget(
        Paragraph::new(info)
            .alignment(Alignment::Right)
            .style(style),
        sections[1],
    );
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
            .map(|(name, _)| {
                ListItem::new(vec![
                    Line::from(format!("  {}", name.to_uppercase())),
                    Line::from(""),
                ])
            })
            .collect()
    } else if app.documents.is_empty() {
        vec![ListItem::new("  No writing yet — Enter creates one")]
    } else {
        app.documents
            .iter()
            .map(|path| {
                let name = if app.category_index == PROJECTS_CATEGORY_INDEX {
                    path.strip_prefix(app.category_directory(PROJECTS_CATEGORY_INDEX))
                        .unwrap_or(path)
                        .with_extension("")
                        .display()
                        .to_string()
                } else {
                    path.file_stem()
                        .and_then(|name| name.to_str())
                        .unwrap_or("Untitled")
                        .to_string()
                };
                let pin = if app.pinned.contains(path) {
                    "★ "
                } else {
                    "  "
                };
                ListItem::new(format!("{pin}{name}"))
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
                .fg(tui_color(palette.background))
                .bg(tui_color(palette.foreground))
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
        if app.category_index == SECRET_CATEGORY_INDEX {
            " ↑↓ choose · Enter unlock · F5 theme · F7/F8 size on Pi · app lock only "
        } else {
            " ↑↓ choose · Enter open · F5 theme · F7/F8 size on Pi · Ctrl+Q quit "
        }
    } else {
        if app.category_index == PROJECTS_CATEGORY_INDEX {
            " ↑↓ open · j book · c chapter · e export · v reference · / search · p pin "
        } else {
            " ↑↓ open · v reference · / search · p pin · r rename · m move · a archive · d trash "
        }
    };
    draw_info_bar(frame, app, footer, chunks[2], palette);
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
            KeyCode::Char('z') => Key::Undo,
            KeyCode::Char('y') => Key::Redo,
            KeyCode::Char('r') => Key::Restore,
            KeyCode::Char('f') => Key::Search,
            KeyCode::Char('g') => Key::Target,
            KeyCode::Char('k') => Key::PreviousHeading,
            KeyCode::Char('j') => Key::NextHeading,
            KeyCode::Up => Key::PreviousHeading,
            KeyCode::Down => Key::NextHeading,
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
        KeyCode::F(3) => Key::FindNext,
        KeyCode::F(4) => Key::Sprint,
        KeyCode::F(5) => Key::Theme,
        KeyCode::F(6) => Key::Rotate,
        KeyCode::F(7) => Key::Larger,
        KeyCode::F(8) => Key::Smaller,
        KeyCode::F(9) => Key::Outline,
        KeyCode::F(10) => Key::SplitView,
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
        app.refresh_network_if_due();
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
                Event::Paste(text) if app.text_prompt.is_some() => {
                    app.prompt_input.push_str(&text);
                    changed = true;
                }
                Event::Paste(text)
                    if app.screen == Screen::Editor
                        && !app.help_visible
                        && app.lock_prompt.is_none() =>
                {
                    app.document.checkpoint();
                    for character in text.chars() {
                        app.document.text.insert(app.document.cursor, character);
                        app.document.cursor += 1;
                    }
                    app.document.dirty = true;
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
    let mut shown_network = app.network_status.load(Ordering::Relaxed);
    app.render_framebuffer(&mut framebuffer)?;
    loop {
        app.refresh_network_if_due();
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
        let network = app.network_status.load(Ordering::Relaxed);
        if network != shown_network {
            shown_network = network;
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

    fn relative_luminance(color: (u8, u8, u8)) -> f64 {
        let channel = |value: u8| {
            let value = f64::from(value) / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(color.0) + 0.7152 * channel(color.1) + 0.0722 * channel(color.2)
    }

    fn contrast_ratio(a: (u8, u8, u8), b: (u8, u8, u8)) -> f64 {
        let (bright, dark) = {
            let a = relative_luminance(a);
            let b = relative_luminance(b);
            if a > b {
                (a, b)
            } else {
                (b, a)
            }
        };
        (bright + 0.05) / (dark + 0.05)
    }

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
    fn undo_and_redo_restore_text_and_cursor() {
        let mut document = Document::from_string("ink".into());
        document.insert('!');
        assert_eq!(document.as_string(), "ink!");
        assert!(document.undo());
        assert_eq!(document.as_string(), "ink");
        assert_eq!(document.cursor, 3);
        assert!(document.redo());
        assert_eq!(document.as_string(), "ink!");
    }

    #[test]
    fn themes_keep_terminal_text_high_contrast() {
        for theme in THEMES {
            for (role, color) in [
                ("foreground", theme.foreground),
                ("muted", theme.muted),
                ("accent", theme.accent),
            ] {
                let ratio = contrast_ratio(color, theme.background);
                assert!(
                    ratio >= 7.0,
                    "{} {role} contrast is only {ratio:.2}:1",
                    theme.name
                );
            }
            let status_ratio = contrast_ratio(theme.foreground, theme.status);
            assert!(
                status_ratio >= 7.0,
                "{} status contrast is only {status_ratio:.2}:1",
                theme.name
            );
            let selection_ratio = contrast_ratio(theme.background, theme.foreground);
            assert!(
                selection_ratio >= 7.0,
                "{} selection contrast is only {selection_ratio:.2}:1",
                theme.name
            );
        }
    }

    #[test]
    fn themes_include_exact_light_and_dark_modes() {
        assert_eq!(THEMES.len(), 3);
        assert_eq!(THEMES[2].name, "Moon Ink");
        assert!(THEMES
            .iter()
            .any(|theme| { theme.background == (0, 0, 0) && theme.foreground == (255, 255, 255) }));
        assert!(THEMES
            .iter()
            .any(|theme| { theme.background == (255, 255, 255) && theme.foreground == (0, 0, 0) }));
    }

    #[test]
    fn wrapping_preserves_every_character() {
        let chars: Vec<char> = "abcdef\ngh".chars().collect();
        let ranges = wrapped_ranges(&chars, 3);
        assert_eq!(ranges, vec![(0, 3), (3, 6), (7, 9)]);
        assert_eq!(cursor_position(&chars, &ranges, 3), (1, 0));
    }

    #[test]
    fn markdown_outline_tracks_heading_levels_and_positions() {
        let text: Vec<char> = "# Novel\nopening\n## Chapter One\ntext\nnot # heading\n"
            .chars()
            .collect();
        let headings = markdown_headings(&text);
        assert_eq!(headings.len(), 2);
        assert_eq!(headings[0], (0, 1, "Novel".into()));
        assert_eq!(headings[1].1, 2);
        assert_eq!(headings[1].2, "Chapter One");
    }

    #[test]
    fn heading_navigation_wraps_through_the_outline() {
        let directory = test_directory("headings");
        let path = directory.join("Drafts/book.md");
        atomic_write(&path, b"# One\ntext\n# Two\nmore").unwrap();
        let mut app = App::open(directory.clone(), Some(path), false).unwrap();
        app.document.cursor = 0;
        app.move_heading(true);
        assert_eq!(app.message, "heading: Two");
        app.move_heading(true);
        assert_eq!(app.document.cursor, 0);
        app.move_heading(false);
        assert_eq!(app.message, "heading: Two");
        fs::remove_dir_all(directory).unwrap();
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
    fn control_arrows_navigate_headings_without_reserved_function_keys() {
        let mut bytes = b"\x1b[1;5A\x1b[1;5B".to_vec();
        assert_eq!(decode_key(&mut bytes), Some(Key::PreviousHeading));
        assert_eq!(decode_key(&mut bytes), Some(Key::NextHeading));
        assert!(bytes.is_empty());
        assert_eq!(
            crossterm_key(KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL)),
            Key::PreviousHeading
        );
        assert_eq!(
            crossterm_key(KeyEvent::new(KeyCode::Down, KeyModifiers::CONTROL)),
            Key::NextHeading
        );
    }

    #[test]
    fn information_bar_reports_connection_without_exceeding_width() {
        let directory = test_directory("info-bar");
        let app = App::open(directory.clone(), None, false).unwrap();
        app.network_status.store(2, Ordering::Relaxed);
        assert!(app.info_bar().ends_with("● online"));
        let line = two_sided_line("shortcuts", &app.info_bar(), 40);
        assert_eq!(visible_width(&line), 40);
        app.network_status.store(1, Ordering::Relaxed);
        assert!(app.info_bar().ends_with("○ offline"));
        fs::remove_dir_all(directory).unwrap();
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
    fn saves_create_recoverable_versions_and_cursor_state() {
        let directory = test_directory("versions");
        let path = directory.join("Notes/draft.md");
        atomic_write(&path, b"first").unwrap();
        let mut app = App::open(directory.clone(), Some(path.clone()), false).unwrap();
        app.document.insert('!');
        app.save().unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "first!");
        let history = directory.join(".quietwrite/history/Notes_draft.md");
        assert_eq!(fs::read_dir(history).unwrap().count(), 1);
        assert_eq!(read_cursor_state(&app.state_path, &path), Some(6));
        app.restore_snapshot().unwrap();
        assert_eq!(app.document.as_string(), "first");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn browser_can_search_pin_archive_and_trash() {
        let directory = test_directory("manage");
        let drafts = directory.join("Drafts");
        atomic_write(&drafts.join("chapter.md"), b"a lighthouse scene").unwrap();
        atomic_write(&drafts.join("other.md"), b"market scene").unwrap();
        let mut app = App::open(directory.clone(), None, false).unwrap();
        app.category_index = 1;
        app.open_category();
        app.shelf_search = "lighthouse".into();
        app.refresh_documents();
        assert_eq!(app.documents.len(), 1);
        app.toggle_pin().unwrap();
        assert_eq!(app.pinned.len(), 1);
        app.move_selected_to(ARCHIVE_CATEGORY_INDEX).unwrap();
        assert!(directory.join("Archive/chapter.md").exists());
        assert_eq!(
            fs::read_to_string(directory.join("Archive/chapter.md")).unwrap(),
            "a lighthouse scene"
        );
        assert_eq!(app.category_index, ARCHIVE_CATEGORY_INDEX);
        assert_eq!(
            app.selected_path(),
            Some(directory.join("Archive/chapter.md"))
        );
        app.category_index = 1;
        app.shelf_search.clear();
        app.refresh_documents();
        app.trash_selected().unwrap();
        assert!(directory.join(".quietwrite/Trash/other.md").exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn projects_discover_nested_chapters_and_open_references() {
        let directory = test_directory("projects");
        let project = directory.join("Projects/Glass House");
        atomic_write(&project.join("01-arrival.md"), b"# Arrival\nDraft").unwrap();
        atomic_write(&project.join("02-letter.md"), b"# Letter\nReference").unwrap();
        let chapters = notes_for_category(&directory, PROJECTS_CATEGORY_INDEX);
        assert_eq!(chapters.len(), 2);
        let mut app = App::open(directory.clone(), None, false).unwrap();
        app.category_index = PROJECTS_CATEGORY_INDEX;
        app.open_category();
        app.document_index = app
            .documents
            .iter()
            .position(|path| path.ends_with("02-letter.md"))
            .unwrap();
        app.select_reference().unwrap();
        assert!(app.split_visible);
        assert_eq!(
            app.reference_text.iter().collect::<String>(),
            "# Letter\nReference"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn project_prompt_creates_a_plain_folder_and_chapter() {
        let directory = test_directory("new-project");
        let mut app = App::open(directory.clone(), None, false).unwrap();
        app.text_prompt = Some(TextPrompt::NewProject);
        app.prompt_input = "Night Train".into();
        app.submit_text_prompt().unwrap();
        let chapter = directory.join("Projects/Night Train/10-chapter-1.md");
        assert!(chapter.exists());
        assert_eq!(app.path, chapter);
        assert!(directory
            .join("Projects/Night Train/00-title-page.md")
            .exists());
        assert!(directory
            .join("Projects/Night Train/01-copyright.md")
            .exists());
        assert!(directory
            .join("Projects/Night Train/90-about-the-author.md")
            .exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn book_export_combines_chapters_in_filename_order() {
        let directory = test_directory("book-export");
        let project = directory.join("Projects/Night Train");
        atomic_write(&project.join("20-ending.md"), b"# Ending\n\nLast line.").unwrap();
        atomic_write(&project.join("10-opening.md"), b"# Opening\n\nFirst line.").unwrap();
        let mut app = App::open(directory.clone(), None, false).unwrap();
        app.category_index = PROJECTS_CATEGORY_INDEX;
        app.open_category();
        app.export_selected_book().unwrap();
        let export = directory.join("Exports/Night Train");
        let markdown = fs::read_to_string(export.join("manuscript.md")).unwrap();
        assert!(markdown.find("First line.").unwrap() < markdown.find("Last line.").unwrap());
        let html = fs::read_to_string(export.join("manuscript.html")).unwrap();
        assert!(html.contains("<h1>Opening</h1>"));
        assert!(html.contains("<title>Night Train</title>"));
        assert!(export.join("KDP-README.txt").exists());
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
        app.category_index = SECRET_CATEGORY_INDEX;
        app.secret_key = Some([7; 32]);
        app.secret_unlocked = true;
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
        let expected = create_secret_lock(&path, b"sample phrase").unwrap();
        assert_eq!(
            unlock_secret(&path, b"sample phrase").unwrap(),
            Some(expected)
        );
        assert_eq!(unlock_secret(&path, b"different phrase").unwrap(), None);
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn secret_ciphertext_round_trips_and_detects_tampering() {
        let key = [42_u8; 32];
        let plaintext = b"a private line of poetry";
        let encrypted = encrypt_secret(&key, plaintext).unwrap();
        assert!(encrypted.starts_with(SECRET_MAGIC));
        assert!(!encrypted
            .windows(plaintext.len())
            .any(|part| part == plaintext));
        assert_eq!(decrypt_secret(&key, &encrypted).unwrap(), plaintext);
        assert!(decrypt_secret(&[41_u8; 32], &encrypted).is_err());
        let mut damaged = encrypted;
        *damaged.last_mut().unwrap() ^= 1;
        assert!(decrypt_secret(&key, &damaged).is_err());
    }

    #[test]
    fn secret_notes_are_encrypted_on_disk_and_history_stays_encrypted() {
        let directory = test_directory("encrypted-save");
        let mut app = App::open(directory.clone(), None, false).unwrap();
        app.category_index = SECRET_CATEGORY_INDEX;
        app.secret_key = Some([9; 32]);
        app.secret_unlocked = true;
        app.new_note().unwrap();
        app.document.insert('x');
        app.save().unwrap();
        let on_disk = fs::read(&app.path).unwrap();
        assert!(on_disk.starts_with(SECRET_MAGIC));
        assert_eq!(app.read_document(&app.path).unwrap(), "x");
        app.document.insert('y');
        app.save().unwrap();
        let history = directory
            .join(".quietwrite/history")
            .join(history_key(&directory, &app.path));
        let snapshot = fs::read(
            fs::read_dir(history)
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path(),
        )
        .unwrap();
        assert!(snapshot.starts_with(SECRET_MAGIC));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn secret_shelf_prompts_and_locks_again_when_leaving() {
        let directory = test_directory("secret-flow");
        let mut app = App::open(directory.clone(), None, false).unwrap();
        app.secret_lock_path = directory.join("secret.lock");
        app.category_index = SECRET_CATEGORY_INDEX;
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
