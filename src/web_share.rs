use super::{hex_decode, hex_encode, ORDINARY_FOLDERS};
use getrandom::fill;
use std::fs;
use std::io::{self, Read};
use std::net::UdpSocket;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::Duration;
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

const PORT: u16 = 8787;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareState {
    pub running: bool,
    pub starting: bool,
    pub url: String,
    pub code: String,
    pub message: String,
}

impl Default for ShareState {
    fn default() -> Self {
        Self {
            running: false,
            starting: false,
            url: String::new(),
            code: String::new(),
            message: "Wi-Fi sharing is off".into(),
        }
    }
}

pub struct WebShare {
    state: Arc<Mutex<ShareState>>,
    stop: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
}

impl WebShare {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(ShareState::default())),
            stop: Arc::new(AtomicBool::new(false)),
            generation: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn snapshot(&self) -> ShareState {
        self.state
            .lock()
            .map(|state| state.clone())
            .unwrap_or_else(|_| ShareState {
                message: "Wi-Fi share state unavailable".into(),
                ..ShareState::default()
            })
    }

    pub fn start(&self, directory: PathBuf) {
        let snapshot = self.snapshot();
        if snapshot.running || snapshot.starting {
            return;
        }
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        self.stop.store(false, Ordering::Relaxed);
        let code = match random_access_code() {
            Ok(code) => code,
            Err(error) => {
                if let Ok(mut state) = self.state.lock() {
                    state.message = format!("Could not create a secure access code: {error}");
                }
                return;
            }
        };
        let session_key = match random_session_key() {
            Ok(key) => key,
            Err(error) => {
                if let Ok(mut state) = self.state.lock() {
                    state.message = format!("Could not create a secure browser session: {error}");
                }
                return;
            }
        };
        let url = share_urls();
        if let Ok(mut state) = self.state.lock() {
            state.starting = true;
            state.running = false;
            state.url = url.clone();
            state.code = code.clone();
            state.message = "Starting read-only Wi-Fi library…".into();
        }
        let state = Arc::clone(&self.state);
        let stop = Arc::clone(&self.stop);
        let active_generation = Arc::clone(&self.generation);
        thread::spawn(move || {
            let result = serve(directory, &code, &session_key, &stop, &state);
            if active_generation.load(Ordering::Relaxed) != generation {
                return;
            }
            if let Ok(mut current) = state.lock() {
                current.running = false;
                current.starting = false;
                current.message = match result {
                    Ok(()) if stop.load(Ordering::Relaxed) => "Wi-Fi sharing stopped".into(),
                    Ok(()) => "Wi-Fi sharing ended".into(),
                    Err(error) => format!("Could not start Wi-Fi sharing: {error}"),
                };
                if !stop.load(Ordering::Relaxed) {
                    current.code.clear();
                }
            }
        });
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        self.generation.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut state) = self.state.lock() {
            state.running = false;
            state.starting = false;
            state.code.clear();
            state.message = "Wi-Fi sharing stopped".into();
        }
    }
}

impl Drop for WebShare {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.generation.fetch_add(1, Ordering::Relaxed);
    }
}

fn random_access_code() -> io::Result<String> {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut bytes = [0_u8; 8];
    fill(&mut bytes).map_err(|error| io::Error::other(error.to_string()))?;
    let raw: String = bytes
        .iter()
        .map(|byte| ALPHABET[*byte as usize % ALPHABET.len()] as char)
        .collect();
    Ok(format!("{}-{}", &raw[..4], &raw[4..]))
}

fn random_session_key() -> io::Result<String> {
    let mut bytes = [0_u8; 24];
    fill(&mut bytes).map_err(|error| io::Error::other(error.to_string()))?;
    Ok(hex_encode(&bytes))
}

fn device_hostname() -> String {
    let value = std::env::var("HOSTNAME")
        .ok()
        .filter(|name| !name.trim().is_empty())
        .or_else(|| {
            Command::new("hostname")
                .output()
                .ok()
                .filter(|output| output.status.success())
                .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        })
        .unwrap_or_else(|| "quietwrite".into());
    let value = value.trim().trim_end_matches(".local");
    let safe: String = value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || *character == '-')
        .collect();
    if safe.is_empty() {
        "quietwrite".into()
    } else {
        safe
    }
}

fn share_urls() -> String {
    let hostname = format!("http://{}.local:{PORT}", device_hostname());
    let address = UdpSocket::bind("0.0.0.0:0")
        .and_then(|socket| {
            socket.connect("192.0.2.1:9")?;
            socket.local_addr()
        })
        .ok()
        .map(|address| address.ip())
        .filter(|address| !address.is_loopback())
        .map(|address| format!("http://{address}:{PORT}"));
    address.map_or(hostname.clone(), |address| {
        if address == hostname {
            hostname
        } else {
            format!("{hostname}  or  {address}")
        }
    })
}

fn serve(
    directory: PathBuf,
    code: &str,
    session_key: &str,
    stop: &AtomicBool,
    state: &Mutex<ShareState>,
) -> io::Result<()> {
    let server =
        Server::http(("0.0.0.0", PORT)).map_err(|error| io::Error::other(error.to_string()))?;
    serve_server(server, directory, code, session_key, stop, state)
}

fn serve_server(
    server: Server,
    directory: PathBuf,
    code: &str,
    session_key: &str,
    stop: &AtomicBool,
    state: &Mutex<ShareState>,
) -> io::Result<()> {
    if let Ok(mut current) = state.lock() {
        current.starting = false;
        current.running = true;
        current.message = "Read-only library is available on the local network".into();
    }
    while !stop.load(Ordering::Relaxed) {
        let Some(request) = server
            .recv_timeout(Duration::from_millis(250))
            .map_err(|error| io::Error::other(error.to_string()))?
        else {
            continue;
        };
        handle_request(request, &directory, code, session_key);
    }
    Ok(())
}

fn handle_request(mut request: Request, directory: &Path, code: &str, session_key: &str) {
    let method = request.method().clone();
    let route = request.url().split('?').next().unwrap_or("/").to_string();
    if method == Method::Post && route == "/login" {
        let mut body = String::new();
        let _ = request.as_reader().take(1024).read_to_string(&mut body);
        let supplied = form_value(&body, "code").unwrap_or_default().to_uppercase();
        if supplied == code.to_uppercase() {
            let header = format!("qw_session={session_key}; HttpOnly; SameSite=Strict; Path=/");
            respond_html(
                request,
                StatusCode(303),
                "<p>Access granted. <a href=\"/\">Open the library</a>.</p>",
                Some(("Set-Cookie", &header)),
                Some(("Location", "/")),
            );
        } else {
            respond_html(request, StatusCode(403), &login_page(true), None, None);
        }
        return;
    }
    if !authenticated(&request, session_key) {
        respond_html(request, StatusCode(401), &login_page(false), None, None);
        return;
    }
    if method != Method::Get {
        respond_text(
            request,
            StatusCode(405),
            "Read-only server: GET requests only",
        );
        return;
    }
    if route == "/" {
        respond_html(request, StatusCode(200), &index_page(directory), None, None);
    } else if let Some(encoded) = route.strip_prefix("/view/") {
        if let Some((path, label)) = resolve_note(directory, encoded) {
            match fs::read_to_string(path) {
                Ok(contents) => respond_html(
                    request,
                    StatusCode(200),
                    &note_page(&label, &contents),
                    None,
                    None,
                ),
                Err(_) => respond_text(request, StatusCode(404), "Note not found"),
            }
        } else {
            respond_text(request, StatusCode(404), "Note not found");
        }
    } else if let Some(encoded) = route.strip_prefix("/download/") {
        if let Some((path, label)) = resolve_note(directory, encoded) {
            match fs::read(path) {
                Ok(contents) => respond_download(request, contents, &label),
                Err(_) => respond_text(request, StatusCode(404), "Note not found"),
            }
        } else {
            respond_text(request, StatusCode(404), "Note not found");
        }
    } else {
        respond_text(request, StatusCode(404), "Not found");
    }
}

fn safe_notes(directory: &Path) -> Vec<(PathBuf, String)> {
    let Ok(root) = fs::canonicalize(directory) else {
        return Vec::new();
    };
    let mut candidates = Vec::new();
    collect_safe_markdown(directory, false, &mut candidates);
    for folder in ORDINARY_FOLDERS {
        collect_safe_markdown(&directory.join(folder), true, &mut candidates);
    }
    let mut notes = Vec::new();
    for path in candidates {
        let Ok(canonical) = fs::canonicalize(&path) else {
            continue;
        };
        let Ok(canonical_relative) = canonical.strip_prefix(&root) else {
            continue;
        };
        if canonical_relative.starts_with("Secret Thoughts") {
            continue;
        }
        let Ok(relative) = path.strip_prefix(directory) else {
            continue;
        };
        if relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        {
            continue;
        }
        let label = relative.to_string_lossy().to_string();
        notes.push((canonical, label));
    }
    notes.sort_by(|left, right| left.1.to_lowercase().cmp(&right.1.to_lowercase()));
    notes.dedup_by(|left, right| left.0 == right.0);
    notes
}

fn collect_safe_markdown(folder: &Path, recursive: bool, notes: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if recursive && kind.is_dir() {
            collect_safe_markdown(&path, true, notes);
        } else if kind.is_file()
            && path.extension().and_then(|extension| extension.to_str()) == Some("md")
        {
            notes.push(path);
        }
    }
}

fn resolve_note(directory: &Path, encoded: &str) -> Option<(PathBuf, String)> {
    let bytes = hex_decode(encoded).ok()?;
    let label = String::from_utf8(bytes).ok()?;
    safe_notes(directory)
        .into_iter()
        .find(|(_, candidate)| candidate == &label)
}

fn index_page(directory: &Path) -> String {
    let notes = safe_notes(directory);
    let note_count = notes.len();
    let entries = if notes.is_empty() {
        "<p class=\"empty\">NO ORDINARY MARKDOWN NOTES FOUND.</p>".into()
    } else {
        notes
            .into_iter()
            .map(|(_, label)| {
                let encoded = hex_encode(label.as_bytes());
                format!(
                    "<li><a href=\"/view/{encoded}\">{}</a><a class=\"download\" href=\"/download/{encoded}\">Download</a></li>",
                    html_escape(&label)
                )
            })
            .collect::<Vec<_>>()
            .join("")
    };
    page(
        "QuietWrite library",
        &format!(
            "<header class=\"masthead\"><div class=\"eyebrow\">LOCAL NETWORK / READ ONLY</div><h1>QUIET<br>WRITE</h1><p>ORDINARY NOTES <strong>{note_count:02} FILES</strong></p></header><main class=\"library\"><div class=\"section-label\">NOTE INDEX</div><ul>{entries}</ul></main>"
        ),
    )
}

fn note_page(label: &str, contents: &str) -> String {
    page(
        label,
        &format!(
            "<nav class=\"topbar\"><a href=\"/\">← ALL NOTES</a><span>QUIETWRITE / VIEWER</span><a href=\"/download/{}\">DOWNLOAD</a></nav><main class=\"document\"><div class=\"section-label\">MARKDOWN / READ ONLY</div><h1>{}</h1><pre>{}</pre></main>",
            hex_encode(label.as_bytes()),
            html_escape(label),
            html_escape(contents)
        ),
    )
}

fn login_page(failed: bool) -> String {
    page(
        "QuietWrite access",
        &format!(
            "<main class=\"login\"><section class=\"login-card\"><div class=\"eyebrow\">PRIVATE DEVICE LINK / 8787</div><h1>QUIET<br>WRITE</h1><p>ENTER THE ACCESS CODE SHOWN ON THE WRITING DEVICE.</p>{}<form method=\"post\" action=\"/login\"><label>ACCESS CODE<input name=\"code\" autocomplete=\"one-time-code\" inputmode=\"text\" autofocus required></label><button type=\"submit\">OPEN LIBRARY →</button></form><p class=\"small\">ORDINARY NOTES ONLY / READ ONLY</p></section></main>",
            if failed { "<p class=\"error\">ACCESS DENIED — CHECK THE CODE.</p>" } else { "" }
        ),
    )
}

const WEB_STYLE: &str = r#"
:root{--paper:#f1efe7;--ink:#111;--white:#fff;--signal:#ff4d28}
*{box-sizing:border-box}
html{background:var(--paper)}
body{margin:0;background:var(--paper);color:var(--ink);font:700 17px/1.4 Arial,Helvetica,sans-serif;min-height:100vh}
a{color:inherit;text-decoration-thickness:2px;text-underline-offset:4px}
a:focus-visible,input:focus-visible,button:focus-visible{outline:3px solid var(--signal);outline-offset:3px}
.masthead{padding:clamp(28px,6vw,72px);border-bottom:4px solid var(--ink);background:var(--white)}
.eyebrow,.section-label{display:inline-block;font:900 12px/1 Arial,Helvetica,sans-serif;letter-spacing:.14em;text-transform:uppercase;border:2px solid var(--ink);padding:8px 10px}
.eyebrow{background:var(--signal)}
.section-label{background:var(--ink);color:var(--white);margin-bottom:20px}
h1{font:900 clamp(52px,12vw,136px)/.78 Impact,Arial Black,Arial,sans-serif;letter-spacing:-.035em;margin:.38em 0 .24em;text-transform:uppercase;overflow-wrap:anywhere}
.masthead p{display:flex;justify-content:space-between;gap:20px;margin:0;border-top:3px solid var(--ink);padding-top:12px;font-size:clamp(13px,2vw,18px);letter-spacing:.08em}
.masthead strong{border:2px solid var(--ink);padding:2px 7px}
.library,.document{max-width:1040px;margin:0 auto;padding:clamp(28px,5vw,60px)}
ul{list-style:none;margin:0;padding:0;border:3px solid var(--ink);background:var(--white)}
li{display:grid;grid-template-columns:minmax(0,1fr) auto;align-items:stretch;border-bottom:2px solid var(--ink)}
li:last-child{border-bottom:0}
li>a:first-child{padding:18px 20px;font-size:clamp(17px,2.5vw,23px);overflow-wrap:anywhere;text-decoration:none}
li>a:first-child:hover{background:#e4e1d7}
a.download{display:flex;align-items:center;border-left:2px solid var(--ink);background:var(--ink);color:var(--white);padding:12px 18px;text-decoration:none;font-size:12px;letter-spacing:.08em}
a.download:hover,.topbar a:hover{background:var(--signal);color:var(--ink)}
.topbar{display:grid;grid-template-columns:1fr auto 1fr;align-items:center;gap:16px;padding:15px 20px;background:var(--ink);color:var(--white);border-bottom:5px solid var(--signal);font-size:12px;letter-spacing:.07em}
.topbar a:last-child{text-align:right}.topbar span{text-align:center;color:#c9c5b9}
.document h1{font-size:clamp(38px,8vw,82px);line-height:.92;margin:.25em 0 .45em;border-bottom:4px solid var(--ink);padding-bottom:20px}
pre{margin:0;white-space:pre-wrap;overflow-wrap:anywhere;background:var(--white);border:3px solid var(--ink);padding:clamp(20px,4vw,36px);font:500 17px/1.7 Courier New,monospace}
.login{min-height:100vh;display:grid;place-items:center;padding:28px;background:var(--paper)}
.login-card{width:min(100%,540px);background:var(--white);border:4px solid var(--ink);box-shadow:9px 9px 0 var(--ink);padding:clamp(26px,6vw,50px)}
.login h1{font-size:clamp(60px,14vw,106px);margin:.4em 0 .3em}
.login p{font-size:14px;letter-spacing:.04em}
form{margin-top:26px;border-top:3px solid var(--ink);padding-top:22px}
label{display:block;font-size:12px;letter-spacing:.12em}
input,button{display:block;width:100%;border:3px solid var(--ink);border-radius:0;font:900 19px/1 Arial,Helvetica,sans-serif}
input{margin:10px 0 14px;padding:15px;background:var(--white);color:var(--ink);text-transform:uppercase;letter-spacing:.15em}
button{margin-top:18px;padding:16px;background:var(--ink);color:var(--white);cursor:pointer;text-align:left}
button:hover{background:var(--signal);color:var(--ink)}
.error{border-left:7px solid var(--signal);padding:8px 12px;background:#eeeae0}.small{margin:26px 0 0;font-size:11px!important}.empty{border:3px solid var(--ink);background:var(--white);padding:28px}
@media(max-width:620px){.masthead{padding:24px}.masthead p{align-items:flex-start;flex-direction:column}.library,.document{padding:24px 18px 36px}li{grid-template-columns:1fr}a.download{justify-content:center;border-left:0;border-top:2px solid var(--ink);padding:12px}.topbar{grid-template-columns:1fr 1fr}.topbar span{display:none}.login{padding:18px}.login-card{box-shadow:6px 6px 0 var(--ink);padding:24px}pre{padding:20px}}
"#;

fn page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta name=\"color-scheme\" content=\"light\"><title>{}</title><style>{}</style></head><body>{body}</body></html>",
        html_escape(title),
        WEB_STYLE
    )
}

fn authenticated(request: &Request, session_key: &str) -> bool {
    request.headers().iter().any(|header| {
        header.field.equiv("Cookie")
            && header
                .value
                .as_str()
                .split(';')
                .any(|cookie| cookie.trim() == format!("qw_session={session_key}"))
    })
}

fn form_value(body: &str, key: &str) -> Option<String> {
    body.split('&').find_map(|field| {
        let (name, value) = field.split_once('=')?;
        (name == key).then(|| percent_decode(value))
    })
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'+' {
            output.push(b' ');
            index += 1;
        } else if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&value[index + 1..index + 3], 16) {
                output.push(byte);
                index += 3;
            } else {
                output.push(bytes[index]);
                index += 1;
            }
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&output).into_owned()
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn content_header(value: &str) -> Header {
    Header::from_bytes("Content-Type", value).expect("static header is valid")
}

fn no_store_header() -> Header {
    Header::from_bytes("Cache-Control", "no-store").expect("static header is valid")
}

fn nosniff_header() -> Header {
    Header::from_bytes("X-Content-Type-Options", "nosniff").expect("static header is valid")
}

fn content_security_header() -> Header {
    Header::from_bytes(
        "Content-Security-Policy",
        "default-src 'self'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'",
    )
    .expect("static header is valid")
}

fn respond_html(
    request: Request,
    status: StatusCode,
    body: &str,
    header: Option<(&str, &str)>,
    second_header: Option<(&str, &str)>,
) {
    let mut response = Response::from_string(body.to_string())
        .with_status_code(status)
        .with_header(content_header("text/html; charset=utf-8"))
        .with_header(no_store_header())
        .with_header(nosniff_header())
        .with_header(content_security_header());
    if let Some((name, value)) = header {
        if let Ok(header) = Header::from_bytes(name, value) {
            response = response.with_header(header);
        }
    }
    if let Some((name, value)) = second_header {
        if let Ok(header) = Header::from_bytes(name, value) {
            response = response.with_header(header);
        }
    }
    let _ = request.respond(response);
}

fn respond_text(request: Request, status: StatusCode, body: &str) {
    let response = Response::from_string(body.to_string())
        .with_status_code(status)
        .with_header(content_header("text/plain; charset=utf-8"))
        .with_header(no_store_header())
        .with_header(nosniff_header());
    let _ = request.respond(response);
}

fn respond_download(request: Request, contents: Vec<u8>, label: &str) {
    let filename: String = Path::new(label)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("note.md")
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, ' ' | '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect();
    let disposition = format!("attachment; filename=\"{filename}\"");
    let response = Response::from_data(contents)
        .with_header(content_header("text/markdown; charset=utf-8"))
        .with_header(no_store_header())
        .with_header(nosniff_header())
        .with_header(
            Header::from_bytes("Content-Disposition", disposition)
                .expect("sanitized header is valid"),
        );
    let _ = request.respond(response);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::{Shutdown, SocketAddr, TcpStream};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_directory(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "quietwrite-web-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn http_request(address: SocketAddr, request: &str) -> String {
        let mut stream = TcpStream::connect(address).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        stream.shutdown(Shutdown::Write).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    }

    #[test]
    fn escaping_and_form_decoding_are_safe() {
        assert_eq!(html_escape("<script>&\""), "&lt;script&gt;&amp;&quot;");
        assert_eq!(
            form_value("code=ABCD-EFGH", "code").as_deref(),
            Some("ABCD-EFGH")
        );
        assert_eq!(percent_decode("A%2DB+C"), "A-B C");
    }

    #[test]
    fn web_pages_use_the_responsive_brutalist_visual_system() {
        let login = login_page(false);
        assert!(login.contains("PRIVATE DEVICE LINK / 8787"));
        assert!(login.contains("--signal:#ff4d28"));
        assert!(login.contains("border:4px solid var(--ink)"));
        assert!(login.contains("box-shadow:9px 9px 0 var(--ink)"));
        assert!(login.contains("@media(max-width:620px)"));
        assert!(login.contains("<html lang=\"en\">"));
        assert!(!login.contains("border-radius:8px"));
        assert!(!login.contains("repeating-linear-gradient"));
        assert!(!login.contains("--acid:"));
    }

    #[test]
    fn safe_notes_excludes_secrets_internal_files_and_outside_symlinks() {
        let directory = test_directory("scope");
        let outside = test_directory("outside");
        fs::create_dir_all(directory.join("Notes")).unwrap();
        fs::create_dir_all(directory.join("Secret Thoughts")).unwrap();
        fs::create_dir_all(directory.join(".quietwrite/Trash")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(directory.join("Notes/visible.md"), "visible").unwrap();
        fs::write(directory.join("Secret Thoughts/private.md"), "ciphertext").unwrap();
        fs::write(directory.join(".quietwrite/Trash/deleted.md"), "deleted").unwrap();
        fs::write(outside.join("outside.md"), "outside").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.join("outside.md"), directory.join("Notes/link.md"))
            .unwrap();

        let notes = safe_notes(&directory);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].1, "Notes/visible.md");
        let encoded = hex_encode(notes[0].1.as_bytes());
        assert!(resolve_note(&directory, &encoded).is_some());
        assert!(resolve_note(&directory, &hex_encode(b"../outside.md")).is_none());
        fs::remove_dir_all(directory).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn access_codes_have_readable_high_entropy_shape() {
        let code = random_access_code().unwrap();
        assert_eq!(code.len(), 9);
        assert_eq!(code.chars().nth(4), Some('-'));
        assert!(code
            .chars()
            .filter(|character| *character != '-')
            .all(|character| character.is_ascii_uppercase() || character.is_ascii_digit()));
        assert_eq!(random_session_key().unwrap().len(), 48);
    }

    #[test]
    fn live_server_requires_code_and_serves_only_safe_notes() {
        let directory = test_directory("server");
        fs::create_dir_all(directory.join("Notes")).unwrap();
        fs::create_dir_all(directory.join("Secret Thoughts")).unwrap();
        fs::write(directory.join("Notes/visible.md"), "a visible sentence").unwrap();
        fs::write(
            directory.join("Secret Thoughts/private.md"),
            "private sentence",
        )
        .unwrap();

        let server = Server::http("127.0.0.1:0").unwrap();
        let address = server.server_addr().to_ip().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let state = Arc::new(Mutex::new(ShareState::default()));
        let code = random_access_code().unwrap();
        let session_key = random_session_key().unwrap();
        let thread_stop = Arc::clone(&stop);
        let thread_state = Arc::clone(&state);
        let thread_directory = directory.clone();
        let thread_code = code.clone();
        let thread_session_key = session_key.clone();
        let handle = thread::spawn(move || {
            serve_server(
                server,
                thread_directory,
                &thread_code,
                &thread_session_key,
                &thread_stop,
                &thread_state,
            )
            .unwrap();
        });

        let unauthenticated = http_request(
            address,
            "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        assert!(unauthenticated.starts_with("HTTP/1.1 401"));
        assert!(!unauthenticated.contains("a visible sentence"));

        let body = format!("code={code}");
        let login = http_request(
            address,
            &format!(
                "POST /login HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            ),
        );
        assert!(login.starts_with("HTTP/1.1 303"));
        assert!(login.contains(&format!("qw_session={session_key}")));

        let cookie = format!("Cookie: qw_session={session_key}");
        let index = http_request(
            address,
            &format!("GET / HTTP/1.1\r\nHost: localhost\r\n{cookie}\r\nConnection: close\r\n\r\n"),
        );
        assert!(index.starts_with("HTTP/1.1 200"));
        assert!(index.contains("Content-Security-Policy:"));
        assert!(index.contains("X-Content-Type-Options: nosniff"));
        assert!(index.contains("Notes/visible.md"));
        assert!(!index.contains("private.md"));

        let encoded = hex_encode(b"Notes/visible.md");
        let download = http_request(
            address,
            &format!(
                "GET /download/{encoded} HTTP/1.1\r\nHost: localhost\r\n{cookie}\r\nConnection: close\r\n\r\n"
            ),
        );
        assert!(download.starts_with("HTTP/1.1 200"));
        assert!(download.contains("a visible sentence"));

        stop.store(true, Ordering::Relaxed);
        handle.join().unwrap();
        fs::remove_dir_all(directory).unwrap();
    }
}
