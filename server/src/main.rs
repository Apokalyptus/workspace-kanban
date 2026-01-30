use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use tiny_http::{Header, Method, Response, Server, StatusCode};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

const DEFAULT_FOLDERS: [(&str, &str); 4] = [
    ("backlog", "Backlog"),
    ("planned", "Planned"),
    ("in_progress", "In Progress"),
    ("done", "Done"),
];
const CONFIG_FILE: &str = ".workspace-kanban";
const THEME_FILE: &str = ".kanban-theme.conf";
#[derive(Debug, Serialize, Deserialize, Clone)]
struct Task {
    id: String,
    title: String,
    description: String,
    creator: String,
    assigned_to: String,
    created_at: String,
    updated_at: String,
    status: String,
    tags: Vec<String>,
    folder: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct BoardColumn {
    id: String,
    title: String,
    wip_limit: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct BoardConfig {
    columns: Vec<BoardColumn>,
}

#[derive(Debug, Serialize)]
struct ThemeSettings {
    headline: Option<String>,
    colors: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct NewTask {
    title: String,
    description: Option<String>,
    creator: Option<String>,
    assigned_to: Option<String>,
    tags: Option<Vec<String>>,
    status: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UpdateTask {
    title: Option<String>,
    description: Option<String>,
    creator: Option<String>,
    assigned_to: Option<String>,
    tags: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct MoveTask {
    folder: String,
}

#[derive(Debug, Deserialize)]
struct BoardUpdate {
    columns: Vec<BoardColumn>,
}

fn now_iso() -> String {
    OffsetDateTime::now_utc().format(&Rfc3339).unwrap_or_default()
}

fn ensure_folders(root: &Path, config: &BoardConfig) -> io::Result<()> {
    for column in &config.columns {
        fs::create_dir_all(root.join(&column.id))?;
    }
    Ok(())
}

fn config_path(root: &Path) -> PathBuf {
    root.join(CONFIG_FILE)
}

fn theme_path(root: &Path) -> PathBuf {
    root.join(THEME_FILE)
}

fn write_default_config(path: &Path) -> io::Result<()> {
    let mut contents = String::new();
    for (id, title) in DEFAULT_FOLDERS {
        contents.push_str(&format!("{}: {}\n", id, title));
    }
    fs::write(path, contents)
}

fn parse_config_line(line: &str) -> Option<BoardColumn> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let (id_part, title_part) = if let Some((left, right)) = trimmed.split_once(':') {
        (left.trim(), right.trim())
    } else {
        (trimmed, trimmed)
    };
    if id_part.is_empty() {
        return None;
    }
    if !id_part
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
    {
        return None;
    }
    let mut title = title_part;
    let mut wip_limit: Option<u32> = None;
    if let Some((base_title, tail)) = title_part.split_once("wip=") {
        title = base_title.trim();
        let raw = tail.trim().split_whitespace().next().unwrap_or("");
        if let Ok(val) = raw.parse::<u32>() {
            if val > 0 {
                wip_limit = Some(val);
            }
        }
    }
    let title = if title.is_empty() {
        id_part
    } else {
        title
    };
    Some(BoardColumn {
        id: id_part.to_string(),
        title: title.to_string(),
        wip_limit,
    })
}

fn load_theme(root: &Path) -> ThemeSettings {
    let path = theme_path(root);
    let mut colors = HashMap::new();
    let mut headline = None;
    if !path.exists() {
        return ThemeSettings { headline, colors };
    }
    if let Ok(contents) = fs::read_to_string(&path) {
        for line in contents.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let (key, value) = match trimmed.split_once('=') {
                Some((k, v)) => (k.trim(), v.trim()),
                None => continue,
            };
            if key.eq_ignore_ascii_case("headline") {
                if !value.is_empty() {
                    headline = Some(value.to_string());
                }
                continue;
            }
            if key.starts_with("color.") && !value.is_empty() {
                colors.insert(key.trim_start_matches("color.").to_string(), value.to_string());
            }
        }
    }
    ThemeSettings { headline, colors }
}

fn write_default_theme(root: &Path) -> io::Result<bool> {
    let path = theme_path(root);
    if path.exists() {
        return Ok(false);
    }
    let contents = "\
# Headline text shown in the app header\n\
headline=Kanban Task Files\n\
\n\
# Primary accent used for buttons\n\
color.accent=#ff7a18\n\
# Darker accent for hover states\n\
color.accent_deep=#c24800\n\
# Main text color\n\
color.ink=#141414\n\
# Muted text and secondary labels\n\
color.muted=#4e4c48\n\
# Card surface color\n\
color.card=#ffffff\n\
# Background gradient start/middle/end\n\
color.bg_start=#fff4e6\n\
color.bg_mid=#f7efe2\n\
color.bg_end=#ece4d7\n";
    fs::write(path, contents)?;
    Ok(true)
}

fn validate_columns(columns: &[BoardColumn]) -> Result<(), String> {
    if columns.is_empty() {
        return Err("Board must have at least one column".to_string());
    }
    let mut seen = HashMap::new();
    for column in columns {
        if column.id.is_empty() {
            return Err("Column id cannot be empty".to_string());
        }
        if !column
            .id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        {
            return Err(format!("Invalid column id: {}", column.id));
        }
        if seen.contains_key(&column.id) {
            return Err(format!("Duplicate column id: {}", column.id));
        }
        seen.insert(column.id.clone(), true);
    }
    Ok(())
}

fn write_config(root: &Path, config: &BoardConfig) -> io::Result<()> {
    let mut contents = String::new();
    for column in &config.columns {
        if let Some(limit) = column.wip_limit {
            if limit > 0 {
                contents.push_str(&format!("{}: {} wip={}\n", column.id, column.title, limit));
                continue;
            }
        }
        contents.push_str(&format!("{}: {}\n", column.id, column.title));
    }
    fs::write(config_path(root), contents)
}

fn load_config(root: &Path, yes: bool) -> io::Result<BoardConfig> {
    let path = config_path(root);
    if !path.exists() {
        if yes {
            write_default_config(&path)?;
        } else {
            println!(
                "Missing {} in {}.",
                CONFIG_FILE,
                root.display()
            );
            print!("Create default board file? [y/N] ");
            io::stdout().flush()?;
            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            let answer = input.trim().to_lowercase();
            if answer == "y" || answer == "yes" {
                write_default_config(&path)?;
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    "Missing .workspace-kanban",
                ));
            }
        }
    }
    let contents = fs::read_to_string(&path)?;
    let mut columns = Vec::new();
    for line in contents.lines() {
        if let Some(column) = parse_config_line(line) {
            columns.push(column);
        }
    }
    if columns.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "No valid columns in .workspace-kanban",
        ));
    }
    Ok(BoardConfig { columns })
}

fn prompt_handle_removed_folder(root: &Path, folder: &str, config: &BoardConfig) -> io::Result<()> {
    let folder_path = root.join(folder);
    let mut tasks = Vec::new();
    if folder_path.exists() {
        for entry in fs::read_dir(&folder_path)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("md") {
                tasks.push(path);
            }
        }
    }
    if tasks.is_empty() {
        let _ = fs::remove_dir_all(&folder_path);
        return Ok(());
    }

    println!(
        "Folder '{}' is not in {} but contains {} task(s).",
        folder,
        CONFIG_FILE,
        tasks.len()
    );
    println!("Choose action: [d]elete tasks, [m]ove tasks, [a]bort");
    print!("> ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let answer = input.trim().to_lowercase();
    match answer.as_str() {
        "d" | "delete" => {
            for path in tasks {
                let _ = fs::remove_file(path);
            }
            let _ = fs::remove_dir_all(&folder_path);
            Ok(())
        }
        "m" | "move" => {
            println!("Move to which folder?");
            for (index, column) in config.columns.iter().enumerate() {
                println!("  {}) {} ({})", index + 1, column.title, column.id);
            }
            print!("> ");
            io::stdout().flush()?;
            let mut choice = String::new();
            io::stdin().read_line(&mut choice)?;
            let idx: usize = choice.trim().parse().unwrap_or(0);
            if idx == 0 || idx > config.columns.len() {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    "Invalid move target",
                ));
            }
            let target = &config.columns[idx - 1].id;
            fs::create_dir_all(root.join(target))?;
            for path in tasks {
                if let Some(filename) = path.file_name() {
                    let dest = root.join(target).join(filename);
                    fs::rename(&path, &dest)?;
                    if let Ok(mut task) = parse_task(&dest, target) {
                        task.folder = target.to_string();
                        task.status = target.to_string();
                        task.updated_at = now_iso();
                        let _ = write_task(&dest, &task);
                    }
                }
            }
            let _ = fs::remove_dir_all(&folder_path);
            Ok(())
        }
        _ => Err(io::Error::new(io::ErrorKind::Other, "Aborted")),
    }
}

fn reconcile_folders(root: &Path, config: &BoardConfig, yes: bool) -> io::Result<()> {
    ensure_folders(root, config)?;
    if !root.exists() {
        return Ok(());
    }
    let mut allowed: HashMap<String, bool> = HashMap::new();
    for column in &config.columns {
        allowed.insert(column.id.clone(), true);
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let folder_name = entry.file_name().to_string_lossy().to_string();
        if folder_name == ".git" {
            continue;
        }
        if !allowed.contains_key(&folder_name) {
            if yes {
                let has_tasks = fs::read_dir(&path)?
                    .filter_map(|e| e.ok())
                    .any(|e| e.path().extension().and_then(|ext| ext.to_str()) == Some("md"));
                if has_tasks {
                    return Err(io::Error::new(
                        io::ErrorKind::Other,
                        format!(
                            "Folder '{}' has tasks but is not in {}; run without -y to resolve",
                            folder_name, CONFIG_FILE
                        ),
                    ));
                } else {
                    let _ = fs::remove_dir_all(&path);
                }
            } else {
                prompt_handle_removed_folder(root, &folder_name, config)?;
            }
        }
    }
    Ok(())
}

fn refresh_config(root: &Path, yes: bool) -> Result<BoardConfig, String> {
    let config = load_config(root, yes).map_err(|err| err.to_string())?;
    reconcile_folders(root, &config, yes).map_err(|err| err.to_string())?;
    Ok(config)
}

fn print_help() {
    println!(r#"Kanban Task Files server

Usage:
  kanban-server [options]

Options:
  -t, --target <dir>             Base directory for task folders (default: ./kanban_data or KANBAN_ROOT)
  -y, --yes                      Create missing folders without prompting
  -h, --help                     Show this help message
      --show-task-editor=<bool>  Show task editor on load (default: true)
      --show-board-editor=<bool> Show board editor on load (default: false)
      --write-default-theme      Create .kanban-theme.conf with default values
      --open-browser=<bool>      Open default system browser on start (default: false)
      --open-browser-once=<bool> Open browser only once per target (default: true)

Environment:
  KANBAN_ROOT   Default base directory if --target is not provided
  KANBAN_PORT   Port to bind (default: 8787)

The server reads .workspace-kanban for board structure and ensures folders exist.
"#);
}

#[derive(Debug, Clone, Copy)]
struct UiOptions {
    show_task_editor: bool,
    show_board_editor: bool,
}

fn parse_args() -> Result<(Option<String>, bool, UiOptions, bool, bool, bool), String> {
    let mut args = std::env::args().skip(1);
    let mut target: Option<String> = None;
    let mut yes = false;
    let mut write_default_settings = false;
    let mut open_browser = false;
    let mut open_browser_once = true;
    let mut ui = UiOptions {
        show_task_editor: true,
        show_board_editor: false,
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-t" | "--target" => {
                let value = args.next().ok_or("Missing value for --target")?;
                target = Some(value);
            }
            "-y" | "--yes" => {
                yes = true;
            }
            "--write-default-theme" => {
                write_default_settings = true;
            }
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            _ if arg.starts_with("--show-task-editor=") => {
                ui.show_task_editor = parse_bool_flag(&arg, "--show-task-editor")?;
            }
            _ if arg.starts_with("--show-board-editor=") => {
                ui.show_board_editor = parse_bool_flag(&arg, "--show-board-editor")?;
            }
            _ if arg.starts_with("--open-browser=") => {
                open_browser = parse_bool_flag(&arg, "--open-browser")?;
            }
            _ if arg.starts_with("--open-browser-once=") => {
                open_browser_once = parse_bool_flag(&arg, "--open-browser-once")?;
            }
            "--show-task-editor" | "--show-board-editor" | "--open-browser" | "--open-browser-once" => {
                return Err("Use --show-task-editor=<true|false>, --show-board-editor=<true|false>, --open-browser=<true|false>, or --open-browser-once=<true|false>".to_string());
            }
            _ => return Err(format!("Unknown argument: {}", arg)),
        }
    }
    Ok((target, yes, ui, write_default_settings, open_browser, open_browser_once))
}
fn parse_bool_flag(arg: &str, name: &str) -> Result<bool, String> {
    let value = arg
        .split_once('=')
        .map(|(_, v)| v)
        .ok_or_else(|| format!("Missing value for {}", name))?;
    match value.to_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(format!("Invalid boolean for {}: {}", name, value)),
    }
}

fn open_browser_url(url: &str) -> io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        Command::new("cmd").args(["/C", "start", "", url]).spawn()?;
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg(url).spawn()?;
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    {
        Command::new("xdg-open").arg(url).spawn()?;
        return Ok(());
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        Err(io::Error::new(
            io::ErrorKind::Other,
            "open browser not supported on this platform",
        ))
    }
}

fn browser_marker_path(root: &Path) -> PathBuf {
    root.join(".kanban-browser-opened")
}

fn slugify(input: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in input.to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_dash = false;
        } else if ch.is_whitespace() || ch == '-' || ch == '_' {
            if !last_dash {
                out.push('-');
                last_dash = true;
            }
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "task".to_string()
    } else {
        trimmed
    }
}

fn is_valid_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn unique_slug(root: &Path, base: &str, config: &BoardConfig) -> String {
    if !exists_anywhere(root, base, config) {
        return base.to_string();
    }
    let mut n = 2;
    loop {
        let candidate = format!("{}-{}", base, n);
        if !exists_anywhere(root, &candidate, config) {
            return candidate;
        }
        n += 1;
    }
}

fn exists_anywhere(root: &Path, id: &str, config: &BoardConfig) -> bool {
    config
        .columns
        .iter()
        .any(|column| root.join(&column.id).join(format!("{}.md", id)).exists())
}

fn task_path(root: &Path, folder: &str, id: &str) -> PathBuf {
    root.join(folder).join(format!("{}.md", id))
}

fn find_task_path(root: &Path, id: &str, config: &BoardConfig) -> Option<(PathBuf, String)> {
    for column in &config.columns {
        let path = task_path(root, &column.id, id);
        if path.exists() {
            return Some((path, column.id.clone()));
        }
    }
    None
}

fn parse_task(path: &Path, folder: &str) -> io::Result<Task> {
    let content = fs::read_to_string(path)?;
    let mut lines = content.lines();
    let mut header: HashMap<String, String> = HashMap::new();
    let mut description_lines: Vec<String> = Vec::new();
    let mut in_body = false;
    while let Some(line) = lines.next() {
        if !in_body {
            if line.trim().is_empty() {
                in_body = true;
                continue;
            }
            if let Some((key, value)) = line.split_once(':') {
                header.insert(key.trim().to_string(), value.trim().to_string());
            }
        } else {
            description_lines.push(line.to_string());
        }
    }
    let file_stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("task");
    let tags = header
        .get("tags")
        .map(|v| {
            v.split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(Task {
        id: file_stem.to_string(),
        title: header.get("title").cloned().unwrap_or_default(),
        description: description_lines.join("\n"),
        creator: header.get("creator").cloned().unwrap_or_default(),
        assigned_to: header.get("assigned_to").cloned().unwrap_or_default(),
        created_at: header.get("created_at").cloned().unwrap_or_default(),
        updated_at: header.get("updated_at").cloned().unwrap_or_default(),
        status: header.get("status").cloned().unwrap_or_else(|| folder.to_string()),
        tags,
        folder: folder.to_string(),
    })
}

fn write_task(path: &Path, task: &Task) -> io::Result<()> {
    let tags = if task.tags.is_empty() {
        String::new()
    } else {
        task.tags.join(", ")
    };
    let body = format!(
        "creator: {}\nassigned_to: {}\ncreated_at: {}\nupdated_at: {}\nstatus: {}\ntags: {}\ntitle: {}\n\n{}\n",
        task.creator,
        task.assigned_to,
        task.created_at,
        task.updated_at,
        task.status,
        tags,
        task.title,
        task.description
    );
    fs::write(path, body)
}

fn load_all_tasks(root: &Path, config: &BoardConfig) -> io::Result<HashMap<String, Vec<Task>>> {
    let mut out: HashMap<String, Vec<Task>> = HashMap::new();
    for column in &config.columns {
        let mut tasks = Vec::new();
        let dir = root.join(&column.id);
        if dir.exists() {
            for entry in fs::read_dir(dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("md") {
                    if let Ok(task) = parse_task(&path, &column.id) {
                        tasks.push(task);
                    }
                }
            }
        }
        out.insert(column.id.clone(), tasks);
    }
    Ok(out)
}

fn content_type_for(path: &str) -> &'static str {
    if path.ends_with(".css") {
        "text/css"
    } else if path.ends_with(".js") {
        "application/javascript"
    } else {
        "text/html"
    }
}

fn respond_json(status: StatusCode, body: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(body)
        .with_status_code(status)
        .with_header(Header::from_bytes("Content-Type", "application/json").unwrap())
}

fn respond_text(status: StatusCode, body: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(body).with_status_code(status)
}

fn main() -> io::Result<()> {
    let (target_arg, yes, ui, write_default_settings_flag, open_browser, open_browser_once) = match parse_args() {
        Ok(v) => v,
        Err(msg) => {
            eprintln!("{}\n", msg);
            print_help();
            std::process::exit(1);
        }
    };
    let port: u16 = std::env::var("KANBAN_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8787);
    let root = target_arg
        .or_else(|| std::env::var("KANBAN_ROOT").ok())
        .unwrap_or_else(|| "./kanban_data".to_string());
    let root_path = PathBuf::from(root);
    if write_default_settings_flag {
        match write_default_theme(&root_path) {
            Ok(true) => println!(
                "Created default theme file at {}",
                theme_path(&root_path).display()
            ),
            Ok(false) => println!(
                "Theme file already exists at {}",
                theme_path(&root_path).display()
            ),
            Err(err) => {
                eprintln!("Failed to write theme: {}", err);
                std::process::exit(1);
            }
        }
    }
    if let Err(msg) = refresh_config(&root_path, yes) {
        eprintln!("{}", msg);
        std::process::exit(1);
    }

    let server = Server::http(("0.0.0.0", port))
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
    let url = format!("http://localhost:{}", port);
    println!("Kanban server running on {}", url);
    if open_browser {
        let marker = browser_marker_path(&root_path);
        let already_opened = open_browser_once && marker.exists();
        if !already_opened {
            if let Err(err) = open_browser_url(&url) {
                eprintln!("Failed to open browser: {}", err);
            } else if open_browser_once {
                let _ = fs::write(marker, url.as_bytes());
            }
        }
    }

    for mut request in server.incoming_requests() {
        let method = request.method().clone();
        let url = request.url().to_string();

        if url.starts_with("/api/") {
            let mut body = String::new();
            let _ = request.as_reader().read_to_string(&mut body);

            let response = match (&method, url.as_str()) {
                (Method::Get, "/api/board") => match refresh_config(&root_path, yes) {
                    Ok(cfg) => {
                        let payload = serde_json::json!({ "board": cfg });
                        respond_json(StatusCode(200), &payload.to_string())
                    }
                    Err(msg) => respond_json(
                        StatusCode(500),
                        &serde_json::json!({"error": msg}).to_string(),
                    ),
                },
                (Method::Put, "/api/board") => match refresh_config(&root_path, yes) {
                    Ok(_cfg) => {
                        let parsed: Result<BoardUpdate, _> = serde_json::from_str(&body);
                        match parsed {
                            Ok(update) => {
                                if let Err(msg) = validate_columns(&update.columns) {
                                    respond_json(
                                        StatusCode(400),
                                        &serde_json::json!({ "error": msg }).to_string(),
                                    )
                                } else {
                                    let new_config = BoardConfig {
                                        columns: update.columns,
                                    };
                                    match write_config(&root_path, &new_config) {
                                        Ok(_) => match refresh_config(&root_path, yes) {
                                            Ok(cfg) => {
                                                let payload = serde_json::json!({ "board": cfg });
                                                respond_json(StatusCode(200), &payload.to_string())
                                            }
                                            Err(msg) => respond_json(
                                                StatusCode(500),
                                                &serde_json::json!({"error": msg}).to_string(),
                                            ),
                                        },
                                        Err(err) => respond_json(
                                            StatusCode(500),
                                            &serde_json::json!({ "error": err.to_string() }).to_string(),
                                        ),
                                    }
                                }
                            }
                            Err(err) => respond_json(
                                StatusCode(400),
                                &serde_json::json!({ "error": err.to_string() }).to_string(),
                            ),
                        }
                    }
                    Err(msg) => respond_json(
                        StatusCode(500),
                        &serde_json::json!({"error": msg}).to_string(),
                    ),
                },
                (Method::Get, "/api/ui") => {
                    let payload = serde_json::json!({
                        "show_task_editor": ui.show_task_editor,
                        "show_board_editor": ui.show_board_editor
                    });
                    respond_json(StatusCode(200), &payload.to_string())
                }
                (Method::Get, "/api/theme") => {
                    let theme = load_theme(&root_path);
                    respond_json(StatusCode(200), &serde_json::json!({ "theme": theme }).to_string())
                }
                (Method::Get, "/api/tasks") => match refresh_config(&root_path, yes) {
                    Ok(cfg) => match load_all_tasks(&root_path, &cfg) {
                            Ok(folders) => {
                                let payload = serde_json::json!({ "folders": folders, "board": cfg });
                                respond_json(StatusCode(200), &payload.to_string())
                            }
                            Err(err) => respond_json(
                                StatusCode(500),
                                &serde_json::json!({"error": err.to_string()}).to_string(),
                            ),
                        },
                    Err(msg) => respond_json(
                        StatusCode(500),
                        &serde_json::json!({"error": msg}).to_string(),
                    ),
                },
                (Method::Post, "/api/tasks") => {
                    match refresh_config(&root_path, yes) {
                        Ok(cfg) => {
                            let parsed: Result<NewTask, _> = serde_json::from_str(&body);
                            match parsed {
                                Ok(new_task) => {
                                    let folder = new_task
                                        .status
                                        .clone()
                                        .filter(|s| cfg.columns.iter().any(|c| c.id == *s))
                                        .unwrap_or_else(|| cfg.columns[0].id.clone());
                                    let base_slug = slugify(&new_task.title);
                                    let id = unique_slug(&root_path, &base_slug, &cfg);
                                    let now = now_iso();
                                    let task = Task {
                                        id: id.clone(),
                                        title: new_task.title,
                                        description: new_task.description.unwrap_or_default(),
                                        creator: new_task.creator.unwrap_or_default(),
                                        assigned_to: new_task.assigned_to.unwrap_or_default(),
                                        created_at: now.clone(),
                                        updated_at: now,
                                        status: folder.clone(),
                                        tags: new_task.tags.unwrap_or_default(),
                                        folder: folder.clone(),
                                    };
                                    let path = task_path(&root_path, &folder, &id);
                                    match write_task(&path, &task) {
                                        Ok(_) => respond_json(
                                            StatusCode(201),
                                            &serde_json::json!(task).to_string(),
                                        ),
                                        Err(err) => respond_json(
                                            StatusCode(500),
                                            &serde_json::json!({ "error": err.to_string() }).to_string(),
                                        ),
                                    }
                                }
                                Err(err) => respond_json(
                                    StatusCode(400),
                                    &serde_json::json!({ "error": err.to_string() }).to_string(),
                                ),
                            }
                        }
                        Err(msg) => respond_json(
                            StatusCode(500),
                            &serde_json::json!({ "error": msg }).to_string(),
                        ),
                    }
                }
                _ => {
                    if let Some(id) = url.strip_prefix("/api/tasks/") {
                        let parts: Vec<&str> = id.split('/').collect();
                        let id_part = parts.first().copied().unwrap_or("");
                        if !is_valid_id(id_part) {
                            respond_json(StatusCode(400), &serde_json::json!({"error": "invalid id"}).to_string())
                        } else if parts.len() == 2 && parts[1] == "move" && method == Method::Post {
                            match refresh_config(&root_path, yes) {
                                Ok(cfg) => {
                                    let parsed: Result<MoveTask, _> = serde_json::from_str(&body);
                                    match parsed {
                                        Ok(move_req) => {
                                            if !cfg.columns.iter().any(|c| c.id == move_req.folder) {
                                                respond_json(StatusCode(400), &serde_json::json!({"error": "invalid folder"}).to_string())
                                            } else if let Some((path, current_folder)) =
                                                find_task_path(&root_path, id_part, &cfg)
                                            {
                                                match parse_task(&path, &current_folder) {
                                                    Ok(mut task) => {
                                                        let target_path = task_path(&root_path, &move_req.folder, id_part);
                                                        if target_path.exists() {
                                                            respond_json(StatusCode(409), &serde_json::json!({"error": "target file exists"}).to_string())
                                                        } else {
                                                            task.folder = move_req.folder.clone();
                                                            task.status = move_req.folder.clone();
                                                            task.updated_at = now_iso();
                                                            if let Err(err) = fs::rename(&path, &target_path) {
                                                                respond_json(StatusCode(500), &serde_json::json!({"error": err.to_string()}).to_string())
                                                            } else if let Err(err) = write_task(&target_path, &task) {
                                                                respond_json(StatusCode(500), &serde_json::json!({"error": err.to_string()}).to_string())
                                                            } else {
                                                                respond_json(StatusCode(200), &serde_json::json!(task).to_string())
                                                            }
                                                        }
                                                    }
                                                    Err(err) => respond_json(StatusCode(500), &serde_json::json!({"error": err.to_string()}).to_string()),
                                                }
                                            } else {
                                                respond_json(StatusCode(404), &serde_json::json!({"error": "task not found"}).to_string())
                                            }
                                        }
                                        Err(err) => respond_json(StatusCode(400), &serde_json::json!({"error": err.to_string()}).to_string()),
                                    }
                                }
                                Err(msg) => respond_json(
                                    StatusCode(500),
                                    &serde_json::json!({ "error": msg }).to_string(),
                                ),
                            }
                        } else if parts.len() == 1 && method == Method::Put {
                            match refresh_config(&root_path, yes) {
                                Ok(cfg) => {
                                    let parsed: Result<UpdateTask, _> = serde_json::from_str(&body);
                                    match parsed {
                                        Ok(update) => {
                                            if let Some((path, folder)) =
                                                find_task_path(&root_path, id_part, &cfg)
                                            {
                                                match parse_task(&path, &folder) {
                                                    Ok(mut task) => {
                                                        let mut rename_error: Option<Response<std::io::Cursor<Vec<u8>>>> = None;
                                                        if let Some(title) = update.title {
                                                            let new_slug = slugify(&title);
                                                            if new_slug != task.id {
                                                                let final_slug =
                                                                    unique_slug(&root_path, &new_slug, &cfg);
                                                                let new_path = task_path(&root_path, &folder, &final_slug);
                                                                if let Err(err) = fs::rename(&path, &new_path) {
                                                                    rename_error = Some(respond_json(
                                                                        StatusCode(500),
                                                                        &serde_json::json!({"error": err.to_string()}).to_string(),
                                                                    ));
                                                                } else {
                                                                    task.id = final_slug;
                                                                }
                                                            }
                                                            task.title = title;
                                                        }
                                                        if let Some(resp) = rename_error {
                                                            resp
                                                        } else {
                                                            if let Some(desc) = update.description {
                                                                task.description = desc;
                                                            }
                                                            if let Some(creator) = update.creator {
                                                                task.creator = creator;
                                                            }
                                                            if let Some(assigned_to) = update.assigned_to {
                                                                task.assigned_to = assigned_to;
                                                            }
                                                            if let Some(tags) = update.tags {
                                                                task.tags = tags;
                                                            }
                                                            task.updated_at = now_iso();
                                                            let final_path = task_path(&root_path, &folder, &task.id);
                                                            match write_task(&final_path, &task) {
                                                                Ok(_) => respond_json(StatusCode(200), &serde_json::json!(task).to_string()),
                                                                Err(err) => respond_json(StatusCode(500), &serde_json::json!({"error": err.to_string()}).to_string()),
                                                            }
                                                        }
                                                    }
                                                    Err(err) => respond_json(StatusCode(500), &serde_json::json!({"error": err.to_string()}).to_string()),
                                                }
                                            } else {
                                                respond_json(StatusCode(404), &serde_json::json!({"error": "task not found"}).to_string())
                                            }
                                        }
                                        Err(err) => respond_json(StatusCode(400), &serde_json::json!({"error": err.to_string()}).to_string()),
                                    }
                                }
                                Err(msg) => respond_json(
                                    StatusCode(500),
                                    &serde_json::json!({ "error": msg }).to_string(),
                                ),
                            }
                        } else if parts.len() == 1 && method == Method::Delete {
                            match refresh_config(&root_path, yes) {
                                Ok(cfg) => {
                                    if let Some((path, _folder)) =
                                        find_task_path(&root_path, id_part, &cfg)
                                    {
                                        match fs::remove_file(&path) {
                                            Ok(_) => respond_json(StatusCode(204), ""),
                                            Err(err) => respond_json(StatusCode(500), &serde_json::json!({"error": err.to_string()}).to_string()),
                                        }
                                    } else {
                                        respond_json(StatusCode(404), &serde_json::json!({"error": "task not found"}).to_string())
                                    }
                                }
                                Err(msg) => respond_json(
                                    StatusCode(500),
                                    &serde_json::json!({ "error": msg }).to_string(),
                                ),
                            }
                        } else {
                            respond_json(StatusCode(404), &serde_json::json!({"error": "not found"}).to_string())
                        }
                    } else {
                        respond_json(StatusCode(404), &serde_json::json!({"error": "not found"}).to_string())
                    }
                }
            };

            let _ = request.respond(response);
            continue;
        }

        let file_path = if url == "/" { "/index.html" } else { url.as_str() };
        let local_path = Path::new("web").join(file_path.trim_start_matches('/'));
        if local_path.exists() && local_path.is_file() {
            match fs::read(&local_path) {
                Ok(data) => {
                    let response = Response::from_data(data)
                        .with_header(Header::from_bytes("Content-Type", content_type_for(file_path)).unwrap());
                    let _ = request.respond(response);
                }
                Err(err) => {
                    let response = respond_text(StatusCode(500), &err.to_string());
                    let _ = request.respond(response);
                }
            }
        } else {
            let response = respond_text(StatusCode(404), "Not Found");
            let _ = request.respond(response);
        }
    }

    Ok(())
}
