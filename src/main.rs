use evdev::{Device, EventType, Key};
use serde::Deserialize;
use std::{
    collections::HashMap,
    env,
    fs,
    os::unix::io::AsRawFd,
    path::PathBuf,
    process,
    thread,
    time::Duration,
};

// Espanso-compatible config format
#[derive(Debug, Deserialize)]
struct EspansoConfig {
    #[serde(default)]
    matches: Vec<Match>,
    #[serde(default)]
    global_vars: Vec<Var>,
}

#[derive(Debug, Deserialize)]
struct Match {
    trigger: Option<String>,
    #[serde(default)]
    triggers: Vec<String>,
    replace: Option<String>,
    #[serde(default)]
    vars: Vec<Var>,
}

#[derive(Debug, Clone, Deserialize)]
struct Var {
    name: String,
    #[serde(rename = "type")]
    var_type: String,
    #[serde(default)]
    params: VarParams,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct VarParams {
    format: Option<String>,
    cmd: Option<String>,
    echo: Option<String>,
}

#[derive(Clone)]
struct Trigger {
    replace: String,
    vars: Vec<Var>,
}

impl Trigger {
    fn expand(&self) -> String {
        let mut result = self.replace.clone();

        for var in &self.vars {
            let value = match var.var_type.as_str() {
                "date" => {
                    let fmt = var.params.format.as_deref().unwrap_or("%Y-%m-%d");
                    run_command("date", &[&format!("+{}", fmt)])
                }
                "shell" => {
                    if let Some(cmd) = &var.params.cmd {
                        run_command("sh", &["-c", cmd])
                    } else {
                        String::new()
                    }
                }
                "clipboard" => run_command("wl-paste", &["-n"]),
                "echo" => var.params.echo.as_ref()
                    .or(var.params.format.as_ref())
                    .cloned()
                    .unwrap_or_default(),
                _ => format!("{{{{{}}}}}", var.name),
            };
            result = result.replace(&format!("{{{{{}}}}}", var.name), &value);
        }
        result
    }
}

fn run_command(cmd: &str, args: &[&str]) -> String {
    process::Command::new(cmd)
        .args(args)
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn key_to_char(key: Key, shift: bool) -> Option<char> {
    let c = match key {
        Key::KEY_A => 'a', Key::KEY_B => 'b', Key::KEY_C => 'c', Key::KEY_D => 'd',
        Key::KEY_E => 'e', Key::KEY_F => 'f', Key::KEY_G => 'g', Key::KEY_H => 'h',
        Key::KEY_I => 'i', Key::KEY_J => 'j', Key::KEY_K => 'k', Key::KEY_L => 'l',
        Key::KEY_M => 'm', Key::KEY_N => 'n', Key::KEY_O => 'o', Key::KEY_P => 'p',
        Key::KEY_Q => 'q', Key::KEY_R => 'r', Key::KEY_S => 's', Key::KEY_T => 't',
        Key::KEY_U => 'u', Key::KEY_V => 'v', Key::KEY_W => 'w', Key::KEY_X => 'x',
        Key::KEY_Y => 'y', Key::KEY_Z => 'z',
        Key::KEY_1 => if shift { '!' } else { '1' },
        Key::KEY_2 => if shift { '@' } else { '2' },
        Key::KEY_3 => if shift { '#' } else { '3' },
        Key::KEY_4 => if shift { '$' } else { '4' },
        Key::KEY_5 => if shift { '%' } else { '5' },
        Key::KEY_6 => if shift { '^' } else { '6' },
        Key::KEY_7 => if shift { '&' } else { '7' },
        Key::KEY_8 => if shift { '*' } else { '8' },
        Key::KEY_9 => if shift { '(' } else { '9' },
        Key::KEY_0 => if shift { ')' } else { '0' },
        Key::KEY_MINUS => if shift { '_' } else { '-' },
        Key::KEY_EQUAL => if shift { '+' } else { '=' },
        Key::KEY_LEFTBRACE => if shift { '{' } else { '[' },
        Key::KEY_RIGHTBRACE => if shift { '}' } else { ']' },
        Key::KEY_SEMICOLON => if shift { ':' } else { ';' },
        Key::KEY_APOSTROPHE => if shift { '"' } else { '\'' },
        Key::KEY_GRAVE => if shift { '~' } else { '`' },
        Key::KEY_BACKSLASH => if shift { '|' } else { '\\' },
        Key::KEY_COMMA => if shift { '<' } else { ',' },
        Key::KEY_DOT => if shift { '>' } else { '.' },
        Key::KEY_SLASH => if shift { '?' } else { '/' },
        Key::KEY_SPACE => ' ',
        _ => return None,
    };
    Some(if shift && c.is_ascii_alphabetic() { c.to_ascii_uppercase() } else { c })
}

fn load_yaml_recursive(dir: &PathBuf, triggers: &mut HashMap<String, Trigger>, global_vars: &mut Vec<Var>) {
    let Ok(entries) = fs::read_dir(dir) else { return };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            load_yaml_recursive(&path, triggers, global_vars);
        } else if path.extension().map_or(false, |e| e == "yaml" || e == "yml") {
            let Ok(content) = fs::read_to_string(&path) else { continue };
            match serde_yaml::from_str::<EspansoConfig>(&content) {
                Ok(config) => {
                    global_vars.extend(config.global_vars);
                    let mut count = 0;
                    for m in config.matches {
                        let Some(replace) = m.replace else { continue };

                        // Collect all triggers: singular `trigger` and plural `triggers`
                        let mut all_triggers = Vec::new();
                        if let Some(t) = m.trigger {
                            all_triggers.push(t);
                        }
                        all_triggers.extend(m.triggers);

                        for trig in all_triggers {
                            triggers.insert(trig, Trigger {
                                replace: replace.clone(),
                                vars: m.vars.clone(),
                            });
                            count += 1;
                        }
                    }
                    if count > 0 {
                        eprintln!("Loaded {} triggers from {:?}", count, path);
                    }
                }
                Err(e) => {
                    eprintln!("Warning: failed to parse {:?}: {}", path, e);
                }
            }
        }
    }
}

fn load_configs() -> HashMap<String, Trigger> {
    let mut triggers = HashMap::new();
    let mut global_vars = Vec::new();
    let config_dir = get_config_path();

    if config_dir.exists() {
        load_yaml_recursive(&config_dir, &mut triggers, &mut global_vars);
    } else {
        eprintln!("Config directory not found: {:?}", config_dir);
    }

    // Prepend global_vars to each trigger's vars (so they're available for expansion)
    if !global_vars.is_empty() {
        for trigger in triggers.values_mut() {
            let mut merged = global_vars.clone();
            merged.extend(trigger.vars.clone());
            trigger.vars = merged;
        }
    }

    triggers
}

fn get_config_path() -> PathBuf {
    let home = env::var("SUDO_USER")
        .ok()
        .and_then(|user| {
            fs::read_to_string("/etc/passwd").ok().and_then(|passwd| {
                passwd.lines()
                    .find(|l| l.starts_with(&format!("{}:", user)))
                    .and_then(|l| l.split(':').nth(5))
                    .map(String::from)
            })
        })
        .or_else(|| env::var("HOME").ok())
        .unwrap_or_else(|| "/tmp".into());

    PathBuf::from(home).join(".config/text_expander")
}

fn find_keyboards() -> Vec<Device> {
    let mut keyboards = Vec::new();
    let mut virtual_kbd = None;

    let Ok(entries) = fs::read_dir("/dev/input") else { return keyboards };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.to_string_lossy().contains("event") { continue }

        let Ok(device) = Device::open(&path) else { continue };

        if !device.supported_events().contains(EventType::KEY) { continue }

        let Some(keys) = device.supported_keys() else { continue };
        if !keys.contains(Key::KEY_A) || !keys.contains(Key::KEY_Z) { continue }

        let name = device.name().unwrap_or("unknown");
        eprintln!("Found keyboard: {:?} - {}", path, name);

        if name.to_lowercase().contains("virtual") {
            virtual_kbd = Some(device);
        } else if virtual_kbd.is_none() {
            keyboards.push(device);
        }
    }

    if let Some(vkbd) = virtual_kbd {
        eprintln!("Using virtual keyboard only (keyd/kmonad detected)");
        vec![vkbd]
    } else {
        keyboards
    }
}

fn get_wayland_env() -> Vec<(String, String)> {
    let mut env_vars = Vec::new();
    let real_uid = env::var("SUDO_UID").unwrap_or_default();

    if let Ok(xdg) = env::var("XDG_RUNTIME_DIR") {
        env_vars.push(("XDG_RUNTIME_DIR".into(), xdg));
    } else if !real_uid.is_empty() {
        env_vars.push(("XDG_RUNTIME_DIR".into(), format!("/run/user/{}", real_uid)));
    }

    env_vars.push(("WAYLAND_DISPLAY".into(),
        env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-1".into())));

    if let Ok(user) = env::var("SUDO_USER") {
        env_vars.push(("USER".into(), user));
    }
    env_vars
}

fn run_wtype(args: &[&str]) {
    if let Ok(sudo_user) = env::var("SUDO_USER") {
        let mut cmd = process::Command::new("sudo");
        cmd.arg("-u").arg(&sudo_user).arg("env");
        for (k, v) in get_wayland_env() {
            cmd.arg(format!("{}={}", k, v));
        }
        cmd.arg("wtype").args(args);
        let _ = cmd.status();
    } else {
        let _ = process::Command::new("wtype").args(args).status();
    }
}

fn type_expansion(backspaces: usize, text: &str) {
    let mut args: Vec<String> = Vec::new();
    for _ in 0..backspaces {
        args.push("-k".into());
        args.push("BackSpace".into());
    }
    args.push("--".into());
    args.push(text.into());

    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    run_wtype(&refs);
}

struct TextExpander {
    triggers: HashMap<String, Trigger>,
    buffer: String,
    max_len: usize,
    shift: bool,
}

impl TextExpander {
    fn new(triggers: HashMap<String, Trigger>) -> Self {
        let max_len = triggers.keys().map(|k| k.len()).max().unwrap_or(64);
        Self { triggers, buffer: String::with_capacity(max_len + 1), max_len, shift: false }
    }

    fn process(&mut self, key: Key, pressed: bool) -> Option<(usize, String)> {
        if key == Key::KEY_LEFTSHIFT || key == Key::KEY_RIGHTSHIFT {
            self.shift = pressed;
            return None;
        }

        if !pressed { return None }

        match key {
            Key::KEY_ENTER | Key::KEY_TAB | Key::KEY_ESC => { self.buffer.clear(); return None }
            Key::KEY_BACKSPACE => { self.buffer.pop(); return None }
            _ => {}
        }

        if let Some(c) = key_to_char(key, self.shift) {
            self.buffer.push(c);
            if self.buffer.len() > self.max_len {
                self.buffer.drain(..self.buffer.len() - self.max_len);
            }

            for (trig, data) in &self.triggers {
                if self.buffer.ends_with(trig) {
                    let result = (trig.len(), data.expand());
                    self.buffer.clear();
                    return Some(result);
                }
            }
        }
        None
    }
}

fn daemonize() {
    // Fork and exit parent
    match unsafe { libc::fork() } {
        -1 => { eprintln!("Fork failed"); process::exit(1); }
        0 => {} // Child continues
        _ => process::exit(0), // Parent exits
    }

    // Create new session
    if unsafe { libc::setsid() } == -1 {
        eprintln!("setsid failed");
        process::exit(1);
    }

    // Redirect stdio to /dev/null
    let devnull = fs::OpenOptions::new()
        .read(true).write(true).open("/dev/null").unwrap();

    unsafe {
        libc::dup2(devnull.as_raw_fd(), 0);
        libc::dup2(devnull.as_raw_fd(), 1);
        libc::dup2(devnull.as_raw_fd(), 2);
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let daemon_mode = args.iter().any(|a| a == "-d" || a == "--daemon");

    eprintln!("text_expander - lightweight espanso replacement for Wayland");

    let triggers = load_configs();
    if triggers.is_empty() {
        eprintln!("No triggers loaded. Create config in ~/.config/text_expander/");
        process::exit(1);
    }
    eprintln!("Loaded {} triggers", triggers.len());

    let mut keyboards = find_keyboards();
    if keyboards.is_empty() {
        eprintln!("No keyboards found. Need read access to /dev/input/*");
        process::exit(1);
    }

    if daemon_mode {
        eprintln!("Daemonizing...");
        daemonize();
    } else {
        eprintln!("Ready! (use -d/--daemon to run in background)");
    }

    let mut expander = TextExpander::new(triggers);
    let raw_fds: Vec<i32> = keyboards.iter().map(|k| k.as_raw_fd()).collect();

    loop {
        let mut pollfds: Vec<libc::pollfd> = raw_fds.iter()
            .map(|&fd| libc::pollfd { fd, events: libc::POLLIN, revents: 0 })
            .collect();

        if unsafe { libc::poll(pollfds.as_mut_ptr(), pollfds.len() as _, -1) } < 0 {
            continue;
        }

        let ready: Vec<usize> = pollfds.iter().enumerate()
            .filter(|(_, p)| p.revents & libc::POLLIN != 0)
            .map(|(i, _)| i).collect();

        let mut expanded = false;

        for i in ready {
            if let Ok(events) = keyboards[i].fetch_events() {
                for ev in events {
                    if ev.event_type() == EventType::KEY {
                        if let Some((n, text)) = expander.process(Key::new(ev.code()), ev.value() == 1) {
                            thread::sleep(Duration::from_millis(10));
                            type_expansion(n, &text);
                            expanded = true;
                        }
                    }
                }
            }
        }

        if expanded {
            thread::sleep(Duration::from_millis(50));
            loop {
                let mut drain: Vec<libc::pollfd> = raw_fds.iter()
                    .map(|&fd| libc::pollfd { fd, events: libc::POLLIN, revents: 0 })
                    .collect();
                if unsafe { libc::poll(drain.as_mut_ptr(), drain.len() as _, 0) } <= 0 { break }
                for (i, p) in drain.iter().enumerate() {
                    if p.revents & libc::POLLIN != 0 {
                        let _ = keyboards[i].fetch_events().map(|e| e.count());
                    }
                }
            }
        }
    }
}
