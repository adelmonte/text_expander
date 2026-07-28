use evdev::{Device, EventType, Key};
use serde::Deserialize;
use std::{
    collections::HashMap,
    env,
    fs,
    io::ErrorKind,
    os::unix::io::AsRawFd,
    path::PathBuf,
    process,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::Duration,
};
use xkbcommon::xkb;

// Espanso-compatible config format
#[derive(Debug, Deserialize)]
struct EspansoConfig {
    #[serde(default)]
    matches: Vec<Match>,
    #[serde(default)]
    global_vars: Vec<Var>,
    keyboard_layout: Option<KeyboardLayout>,
}

// espanso's `keyboard_layout` block. Empty strings are meaningful: libxkbcommon falls back to the
// XKB_DEFAULT_* env vars, then to US.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
struct KeyboardLayout {
    #[serde(default, alias = "rules")]
    rule: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    layout: String,
    #[serde(default)]
    variant: String,
    options: Option<String>,
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

// evdev reports physical key positions, not characters. Translating them requires the active
// keymap: on a de/neo layout, physical KEY_L produces 't', so a hardcoded US table decodes most
// keys wrong. libxkbcommon compiles the real keymap and tracks modifiers, which also makes Neo's
// Mod3/Mod5 levels work for free.
struct Decoder {
    state: xkb::State,
}

impl Decoder {
    fn new(cfg: &KeyboardLayout) -> Option<Self> {
        let ctx = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap = xkb::Keymap::new_from_names(
            &ctx,
            cfg.rule.as_str(),
            cfg.model.as_str(),
            cfg.layout.as_str(),
            cfg.variant.as_str(),
            cfg.options.clone(),
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )?;
        Some(Self { state: xkb::State::new(&keymap) })
    }

    // `value` is the raw evdev value: 0 release, 1 press, 2 autorepeat.
    // Returns the text produced by a press, if any.
    fn feed(&mut self, code: u16, value: i32) -> Option<String> {
        // xkb keycodes are evdev codes offset by 8.
        let keycode = xkb::Keycode::new(code as u32 + 8);

        match value {
            // Read the symbol before updating state, so a locked/latched modifier (Neo puts its
            // level-3 switch on CapsLock) does not apply to the keypress that set it.
            1 => {
                let text = self.state.key_get_utf8(keycode);
                self.state.update_key(keycode, xkb::KeyDirection::Down);
                Some(text)
            }
            0 => {
                self.state.update_key(keycode, xkb::KeyDirection::Up);
                None
            }
            // Autorepeat. Must not reach update_key, which would register as a release and
            // desynchronise the modifier state.
            _ => None,
        }
    }

    // Ctrl/Alt/Super combinations are shortcuts rather than text. Neo's level switches are
    // Mod3/Mod5, so its layers are deliberately not treated as modifiers here.
    fn shortcut_active(&self) -> bool {
        [xkb::MOD_NAME_CTRL, xkb::MOD_NAME_ALT, xkb::MOD_NAME_LOGO]
            .iter()
            .any(|m| self.state.mod_name_is_active(*m, xkb::STATE_MODS_EFFECTIVE))
    }
}

#[derive(Default)]
struct Configs {
    triggers: HashMap<String, Trigger>,
    global_vars: Vec<Var>,
    layout: Option<KeyboardLayout>,
}

fn load_yaml_recursive(dir: &PathBuf, configs: &mut Configs) {
    let Ok(entries) = fs::read_dir(dir) else { return };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            load_yaml_recursive(&path, configs);
        } else if path.extension().map_or(false, |e| e == "yaml" || e == "yml") {
            let Ok(content) = fs::read_to_string(&path) else { continue };
            match serde_yaml::from_str::<EspansoConfig>(&content) {
                Ok(config) => {
                    configs.global_vars.extend(config.global_vars);

                    // read_dir order is arbitrary, so keep the first layout found and only
                    // complain if a later file disagrees.
                    if let Some(layout) = config.keyboard_layout {
                        match &configs.layout {
                            None => {
                                eprintln!("Keyboard layout from {:?}: {:?}", path, layout);
                                configs.layout = Some(layout);
                            }
                            Some(existing) if *existing != layout => {
                                eprintln!("Warning: ignoring conflicting keyboard_layout in {:?}", path);
                            }
                            Some(_) => {}
                        }
                    }

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
                            configs.triggers.insert(trig, Trigger {
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

fn load_configs() -> Configs {
    let mut configs = Configs::default();
    let config_dir = get_config_path();

    if config_dir.exists() {
        load_yaml_recursive(&config_dir, &mut configs);
    } else {
        eprintln!("Config directory not found: {:?}", config_dir);
    }

    // Prepend global_vars to each trigger's vars (so they're available for expansion)
    if !configs.global_vars.is_empty() {
        let global_vars = configs.global_vars.clone();
        for trigger in configs.triggers.values_mut() {
            let mut merged = global_vars.clone();
            merged.extend(trigger.vars.clone());
            trigger.vars = merged;
        }
    }

    configs
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

// With `virtual_only`, collapse to the single virtual keyboard a remapper (keyd/kmonad) exposes,
// since it replays every real keystroke. Off by default: output-only virtual devices such as
// ydotoold's also match the name, and preferring one of those means reading a device that never
// emits the user's typing.
fn find_keyboards(virtual_only: bool) -> Vec<Device> {
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

        if virtual_only && name.to_lowercase().contains("virtual") {
            virtual_kbd = Some(device);
        } else {
            keyboards.push(device);
        }
    }

    if !virtual_only {
        return keyboards;
    }

    match virtual_kbd {
        Some(vkbd) => {
            eprintln!("Using virtual keyboard only (--virtual-only)");
            vec![vkbd]
        }
        None => {
            eprintln!("Warning: --virtual-only given but no virtual keyboard found, using all keyboards");
            keyboards
        }
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

static MISSING_WARNED: AtomicBool = AtomicBool::new(false);

fn run_wtype(args: &[&str]) {
    let (bin, mut cmd) = match env::var("SUDO_USER") {
        Ok(sudo_user) => {
            let mut cmd = process::Command::new("sudo");
            cmd.arg("-u").arg(&sudo_user).arg("env");
            for (k, v) in get_wayland_env() {
                cmd.arg(format!("{}={}", k, v));
            }
            cmd.arg("wtype").args(args);
            ("sudo", cmd)
        }
        Err(_) => {
            let mut cmd = process::Command::new("wtype");
            cmd.args(args);
            ("wtype", cmd)
        }
    };

    // Never fatal: a failed expansion should not take down a running daemon.
    match cmd.status() {
        Ok(status) if status.success() => {}
        Ok(status) => eprintln!("{} exited with {}", bin, status),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            if !MISSING_WARNED.swap(true, Ordering::Relaxed) {
                eprintln!("{} not found - expansions cannot be typed. Install wtype.", bin);
            }
        }
        Err(e) => eprintln!("Failed to run {}: {}", bin, e),
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
    decoder: Decoder,
    buffer: String,
    // Counted in characters, not bytes: a de/neo layout produces multi-byte text.
    max_len: usize,
    debug_keys: bool,
}

impl TextExpander {
    fn new(triggers: HashMap<String, Trigger>, decoder: Decoder, debug_keys: bool) -> Self {
        let max_len = triggers.keys().map(|k| k.chars().count()).max().unwrap_or(64);
        Self {
            triggers,
            decoder,
            buffer: String::with_capacity((max_len + 1) * 4),
            max_len,
            debug_keys,
        }
    }

    // Returns (backspaces, replacement) when a trigger fires. `value` is the raw evdev value.
    fn process(&mut self, code: u16, value: i32) -> Option<(usize, String)> {
        let text = self.decoder.feed(code, value)?;

        match Key::new(code) {
            Key::KEY_ENTER | Key::KEY_TAB | Key::KEY_ESC => { self.buffer.clear(); return None }
            Key::KEY_BACKSPACE => { self.buffer.pop(); return None }
            _ => {}
        }

        // Ctrl+A and friends are commands, and may move the cursor; the buffer no longer
        // reflects what is on screen.
        if self.decoder.shortcut_active() {
            self.buffer.clear();
            return None;
        }

        // Empty for dead keys and non-text keys such as arrows. Control characters are dropped so
        // the likes of "\r" cannot enter the buffer.
        for c in text.chars().filter(|c| !c.is_control()) {
            self.buffer.push(c);
        }

        while self.buffer.chars().count() > self.max_len {
            self.buffer.remove(0);
        }

        if self.debug_keys && !text.is_empty() {
            eprintln!("key code {} -> {:?} | buffer {:?}", code, text, self.buffer);
        }

        for (trig, data) in &self.triggers {
            if self.buffer.ends_with(trig) {
                // Backspaces are a count of characters, not bytes.
                let result = (trig.chars().count(), data.expand());
                self.buffer.clear();
                return Some(result);
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
    let virtual_only = args.iter().any(|a| a == "--virtual-only");
    let debug_keys = args.iter().any(|a| a == "--debug-keys");

    eprintln!("text_expander - lightweight espanso replacement for Wayland");

    let configs = load_configs();
    if configs.triggers.is_empty() {
        eprintln!("No triggers loaded. Create config in ~/.config/text_expander/");
        process::exit(1);
    }
    eprintln!("Loaded {} triggers", configs.triggers.len());

    let layout = configs.layout.unwrap_or_else(|| {
        eprintln!("Warning: no keyboard_layout configured, falling back to XKB_DEFAULT_* or US.");
        eprintln!("         Set it in config/default.yml if your layout is not US.");
        KeyboardLayout::default()
    });

    // Exit rather than falling back to US: a silently wrong layout decodes every keystroke to the
    // wrong character, which looks like triggers simply not working.
    let Some(decoder) = Decoder::new(&layout) else {
        eprintln!("Failed to compile keymap for rule={:?} model={:?} layout={:?} variant={:?} options={:?}",
            layout.rule, layout.model, layout.layout, layout.variant, layout.options);
        process::exit(1);
    };

    let mut keyboards = find_keyboards(virtual_only);
    if keyboards.is_empty() {
        eprintln!("No keyboards found. Need read access to /dev/input/* (join the 'input' group)");
        process::exit(1);
    }

    if daemon_mode {
        eprintln!("Daemonizing...");
        daemonize();
    } else {
        eprintln!("Ready! (use -d/--daemon to run in background)");
    }

    let mut expander = TextExpander::new(configs.triggers, decoder, debug_keys);

    loop {
        let raw_fds: Vec<i32> = keyboards.iter().map(|k| k.as_raw_fd()).collect();
        let mut pollfds: Vec<libc::pollfd> = raw_fds.iter()
            .map(|&fd| libc::pollfd { fd, events: libc::POLLIN, revents: 0 })
            .collect();

        if unsafe { libc::poll(pollfds.as_mut_ptr(), pollfds.len() as _, -1) } < 0 {
            continue;
        }

        let mut i = pollfds.len();
        while i > 0 {
            i -= 1;
            if pollfds[i].revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
                eprintln!("Keyboard disconnected (fd {}), removing", raw_fds[i]);
                keyboards.remove(i);
            }
        }
        if keyboards.is_empty() {
            eprintln!("All keyboards disconnected, exiting");
            process::exit(0);
        }

        let ready: Vec<usize> = pollfds.iter().enumerate()
            .filter(|(_, p)| p.revents & libc::POLLIN != 0)
            .map(|(i, _)| i).collect();

        let mut expanded = false;

        for &i in &ready {
            if i >= keyboards.len() { continue }
            if let Ok(events) = keyboards[i].fetch_events() {
                for ev in events {
                    if ev.event_type() == EventType::KEY {
                        if let Some((n, text)) = expander.process(ev.code(), ev.value()) {
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
            let drain_fds: Vec<i32> = keyboards.iter().map(|k| k.as_raw_fd()).collect();
            loop {
                let mut drain: Vec<libc::pollfd> = drain_fds.iter()
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

#[cfg(test)]
mod tests {
    use super::*;

    // evdev codes, i.e. physical key positions as labelled on a US keyboard.
    const KEY_T: u16 = 20;
    const KEY_S: u16 = 31;
    const KEY_H: u16 = 35;
    const KEY_L: u16 = 38;
    const KEY_Z: u16 = 44;
    const KEY_M: u16 = 50;
    const KEY_DOT: u16 = 52;

    fn layout(layout: &str, variant: &str) -> KeyboardLayout {
        KeyboardLayout { layout: layout.into(), variant: variant.into(), ..Default::default() }
    }

    fn decoder(l: &KeyboardLayout) -> Decoder {
        Decoder::new(l).expect("keymap should compile (needs xkb data in /usr/share/X11/xkb)")
    }

    fn press(d: &mut Decoder, code: u16) -> String {
        let text = d.feed(code, 1).unwrap_or_default();
        d.feed(code, 0);
        text
    }

    // The bug this replaced a hardcoded US table for: physical KEY_L is 't' on de/neo, so typing
    // "ts" produced "lh" and never matched a "ts.." trigger.
    #[test]
    fn decodes_neo_layout_by_position() {
        let mut d = decoder(&layout("de", "neo"));
        assert_eq!(press(&mut d, KEY_L), "t");
        assert_eq!(press(&mut d, KEY_H), "s");
        assert_eq!(press(&mut d, KEY_T), "w");
        assert_eq!(press(&mut d, KEY_S), "i");
        // Identical in both layouts, which is why "m.." was the one trigger that worked.
        assert_eq!(press(&mut d, KEY_M), "m");
        assert_eq!(press(&mut d, KEY_DOT), ".");
    }

    #[test]
    fn decodes_us_layout_by_position() {
        let mut d = decoder(&layout("us", ""));
        assert_eq!(press(&mut d, KEY_L), "l");
        assert_eq!(press(&mut d, KEY_T), "t");
    }

    #[test]
    fn autorepeat_does_not_desync_modifiers() {
        const KEY_LEFTSHIFT: u16 = 42;
        let mut d = decoder(&layout("us", ""));
        d.feed(KEY_LEFTSHIFT, 1);
        // Value 2 is autorepeat. Treating it as a release would drop shift.
        d.feed(KEY_LEFTSHIFT, 2);
        assert_eq!(d.feed(KEY_T, 1), Some("T".into()));
    }

    fn expander(triggers: &[(&str, &str)], l: &KeyboardLayout) -> TextExpander {
        let map = triggers.iter()
            .map(|(t, r)| ((*t).into(), Trigger { replace: (*r).into(), vars: vec![] }))
            .collect();
        TextExpander::new(map, decoder(l), false)
    }

    #[test]
    fn fires_trigger_typed_in_neo() {
        let neo = layout("de", "neo");
        let mut e = expander(&[("ts..", "expanded")], &neo);

        // Physical keys a Neo typist presses for "ts..".
        for code in [KEY_L, KEY_H, KEY_DOT] {
            assert!(e.process(code, 1).is_none());
            e.process(code, 0);
        }
        assert_eq!(e.process(KEY_DOT, 1), Some((4, "expanded".into())));
    }

    #[test]
    fn does_not_fire_on_us_positions() {
        let neo = layout("de", "neo");
        let mut e = expander(&[("ts..", "expanded")], &neo);

        // Typing at the QWERTY positions for "ts" used to trigger this by mistake.
        for code in [KEY_T, KEY_S, KEY_DOT, KEY_DOT] {
            assert_eq!(e.process(code, 1), None);
            e.process(code, 0);
        }
    }

    // Multi-byte characters used to be trimmed by byte index, panicking mid-character.
    #[test]
    fn multibyte_buffer_does_not_panic() {
        let neo = layout("de", "neo");
        // Physical KEY_Z is 'ü' on neo: two bytes, one character.
        assert_eq!(press(&mut decoder(&neo), KEY_Z), "ü");

        let mut e = expander(&[("teamsen..", "x")], &neo);
        for _ in 0..40 {
            assert_eq!(e.process(KEY_Z, 1), None);
            e.process(KEY_Z, 0);
        }
        assert_eq!(e.buffer.chars().count(), 9);
    }

    #[test]
    fn backspace_count_is_characters_not_bytes() {
        let neo = layout("de", "neo");
        let mut e = expander(&[("üü", "x")], &neo);

        assert_eq!(e.process(KEY_Z, 1), None);
        e.process(KEY_Z, 0);
        // "üü" is 4 bytes but must delete only 2 typed characters.
        assert_eq!(e.process(KEY_Z, 1), Some((2, "x".into())));
    }
}
