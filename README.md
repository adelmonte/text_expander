# text_expander

Lightweight text expander for Wayland. Built as a minimal replacement for [espanso](https://espanso.org/) that reads espanso-format config files.

Supports the most commonly used espanso match features (simple triggers, variables, shell commands). Advanced features like regex triggers, forms, and app-specific configs are not supported.

## Requirements

- Linux + Wayland
- `wtype` (text injection)
- `wl-paste` (clipboard variable support)
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
```

### `--virtual-only`

Key remappers like [keyd](https://github.com/rvaiya/keyd) and
[kmonad](https://github.com/kmonad/kmonad) expose a virtual keyboard that replays every real
keystroke. On those setups, pass `--virtual-only` to read that device alone and avoid seeing each
keystroke twice.

Do not pass it otherwise. Output-only virtual devices — such as the one `ydotoold` creates — also
have "virtual" in their name but never emit your typing, so preferring one means no trigger ever
matches. Without the flag, all detected keyboards are read.

## Config

Location: `~/.config/text_expander/`

All `.yml` and `.yaml` files are loaded recursively.

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

1. Reads keyboard input via evdev (all keyboards, or one virtual device with `--virtual-only`)
2. Buffers keystrokes and matches against triggers
3. On match: sends backspaces to delete trigger, types replacement via `wtype`

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
