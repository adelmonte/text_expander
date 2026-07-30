# text_expander

Lightweight text expander for Wayland. Built as a minimal replacement for [espanso](https://espanso.org/) that reads espanso-format config files.

Supports the most commonly used espanso match features (simple triggers, variables, shell commands). Advanced features like regex triggers, forms, and app-specific configs are not supported.

## Requirements

- Linux + Wayland
- `wtype` (text injection)
- `wl-paste` (clipboard variable support)
- `libxkbcommon` (keyboard layout decoding)
- Read access to `/dev/input/event*`

Root is not required. On most distros `/dev/input/event*` is owned by the `input` group, so
joining it is enough:

```bash
sudo usermod -aG input "$USER"
```

Log out and back in for the new group to apply (`id` should list `input`).

## Build

```bash
cargo build --release
sudo cp target/release/text_expander /usr/local/bin/
```

## Usage

```bash
text_expander                  # foreground
text_expander -d               # daemon mode
text_expander --virtual-only   # keyd/kmonad setups (see below)
text_expander --debug-keys     # print each decoded character and the trigger buffer
```

Use `--debug-keys` when triggers do not fire: it shows what the program thinks you typed, which
immediately reveals a wrong keyboard layout.

### `--virtual-only`

Key remappers like [keyd](https://github.com/rvaiya/keyd) and
[kmonad](https://github.com/kmonad/kmonad) expose a virtual keyboard that replays every real
keystroke. On those setups, pass `--virtual-only` to read that device alone and avoid seeing each
keystroke twice.

Do not pass it otherwise. Output-only virtual devices — such as the one `ydotoold` creates — also
have "virtual" in their name but never emit your typing, so preferring one means no trigger ever
matches. Without the flag, all detected keyboards are read.

If the remapper starts after `text_expander`, its virtual keyboard is picked up on the next rescan
(see below) and the real keyboards are dropped at that point.

### Hotplug

`/dev/input` is rescanned about once a second, so keyboards plugged in while the program runs start
working within a second. Unplugging is handled too: the device is dropped and, if it was the last
one, the program waits for a keyboard to reappear instead of exiting.

At least one readable keyboard must be present at startup — otherwise it exits with the
`input`-group hint, since that is nearly always a permissions problem rather than an empty machine.

## Config

Location: `~/.config/text_expander/`

All `.yml` and `.yaml` files are loaded recursively.

### Keyboard layout (required for non-US layouts)

evdev reports *physical key positions*, not characters, so the active keymap is needed to know what
you typed. Set it in `config/default.yml`, using espanso's `keyboard_layout` block:

```yaml
keyboard_layout:
  layout: "de"
  variant: "neo"
  # optional, all default to empty (libxkbcommon's own defaults)
  # rule: ""
  # model: "pc105"
  # options: "grp:alt_shift_toggle"
```

Resolved by libxkbcommon against the same data X and Wayland compositors use, so any
layout/variant in `/usr/share/X11/xkb/symbols/` works. Modifier levels come along for free —
including multi-level layouts like Neo, whose level 3-6 symbols and home-row numpad decode
correctly.

If omitted, the `XKB_DEFAULT_LAYOUT`/`XKB_DEFAULT_VARIANT` environment variables are used, falling
back to US. A wrong layout means triggers silently never match, so a warning is printed when
nothing is configured, and an unknown layout name is a hard error rather than a silent US fallback.

**Layout switching is not tracked.** The configured layout is always used. If you have several
groups configured (e.g. `layout: "de,de"`) and switch between them, triggers only match while the
configured one is active.

### Syntax (espanso-compatible)

```yaml
matches:
  # Simple replacement
  - trigger: ":sig"
    replace: "Best regards,\nJohn"

  # Multiple triggers for one replacement
  - triggers: [":hi", ":hello"]
    replace: "Hello there!"

  # Date variable
  - trigger: ":date"
    replace: "{{date}}"
    vars:
      - name: date
        type: date
        params:
          format: "%Y-%m-%d"

  # Shell command
  - trigger: ":ip"
    replace: "{{ip}}"
    vars:
      - name: ip
        type: shell
        params:
          cmd: "curl -s ifconfig.me"

  # Clipboard
  - trigger: ":paste"
    replace: "{{clip}}"
    vars:
      - name: clip
        type: clipboard
```

### Variable Types

| Type | Params | Description |
|------|--------|-------------|
| `date` | `format` | strftime format string |
| `shell` | `cmd` | Shell command output |
| `clipboard` | - | Current clipboard content (via `wl-paste`) |
| `echo` | `echo` | Static text |

### Supported espanso Features

- `trigger` (single string) and `triggers` (array of strings)
- `replace` with `{{variable}}` interpolation
- `vars` with `date`, `shell`, `clipboard`, and `echo` types
- `global_vars` for shared variables across matches
- Recursive YAML file loading

### Not Supported

These espanso features are intentionally out of scope for this minimal tool:

- Regex triggers, word boundaries, case propagation
- Forms, choice dialogs, cursor hints (`$|$`)
- Rich text (markdown/HTML), image pasting
- App-specific configs, toggle key, search bar
- Config options (backend, clipboard_threshold, etc.)
- `random`, `script`, `match` variable types

## Migrating from espanso

```bash
# Stop espanso
systemctl --user stop espanso

# Copy config
mkdir -p ~/.config/text_expander
cp -r ~/.config/espanso/* ~/.config/text_expander/

# Remove espanso (optional)
rm -rf ~/.config/espanso
```

Simple trigger/replace matches and basic variable types will work as-is. Matches using unsupported features (regex, forms, etc.) will be silently skipped.

## How It Works

1. Reads keyboard input via evdev (all keyboards, or one virtual device with `--virtual-only`),
   rescanning `/dev/input` once a second so devices can come and go
2. Decodes keycodes into characters with libxkbcommon, using the configured layout
3. Buffers characters and matches against triggers
4. On match: sends backspaces to delete trigger, types replacement via `wtype`

## Systemd Service

Run as a user service so it inherits your Wayland session. `~/.config/systemd/user/text_expander.service`:

```ini
[Unit]
Description=Text Expander
After=graphical-session.target

[Service]
ExecStart=/usr/local/bin/text_expander
Restart=always

[Install]
WantedBy=graphical-session.target
```

```bash
systemctl --user enable --now text_expander
```

## License

[GPL-3.0](LICENSE)
