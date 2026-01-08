# text_expander

Lightweight text expander for Wayland. Built as a minimal replacement for [espanso](https://espanso.org/) with full config compatibility.

## Requirements

- Linux + Wayland
- `wtype` (text injection)
- Root access for `/dev/input/event*`

## Build

```bash
cargo build --release
sudo cp target/release/text_expander /usr/local/bin/
```

## Usage

```bash
sudo text_expander        # foreground
sudo text_expander -d     # daemon mode
```

## Config

Location: `~/.config/text_expander/`

All `.yml` and `.yaml` files are loaded recursively.

### Syntax (espanso-compatible)

```yaml
matches:
  # Simple replacement
  - trigger: ":sig"
    replace: "Best regards,\nJohn"

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
| `clipboard` | - | Current clipboard content |
| `echo` | `format` | Static text |

## Migrating from espanso

```bash
# Stop espanso
systemctl --user stop espanso

# Move config
mkdir -p ~/.config/text_expander
cp -r ~/.config/espanso/* ~/.config/text_expander/

# Remove espanso (optional)
rm -rf ~/.config/espanso
```

Your existing espanso match files work without modification.

## How It Works

1. Reads keyboard input via evdev (prefers virtual keyboards like keyd/kmonad)
2. Buffers keystrokes and matches against triggers
3. On match: sends backspaces to delete trigger, types replacement via `wtype`

## Systemd Service

`/etc/systemd/system/text_expander.service`:

```ini
[Unit]
Description=Text Expander
After=graphical.target

[Service]
ExecStart=/usr/local/bin/text_expander
Restart=always
Environment=SUDO_USER=yourusername
Environment=SUDO_UID=1000

[Install]
WantedBy=graphical.target
```

```bash
sudo systemctl enable --now text_expander
```

## License

GPL-3.0
