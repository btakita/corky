# Watch Daemon

Run as a daemon to poll IMAP and Gmail API accounts, sync threads, and push to shared repos automatically.

## Usage

```sh
# Interactive — polls every 5 minutes (default), Ctrl-C to stop
corky watch

# Custom interval
corky watch --interval 60
```

## Configuration

Configure in `mail/.corky.toml`:

```toml
[watch]
poll_interval = 300    # seconds between polls (default: 300)
notify = true          # desktop alerts on new messages (default: false)
```

CLI `--interval` overrides config.

## Notifications

- macOS: `osascript -e 'display notification ...'`
- Linux: `notify-desktop`, falling back to `notify-send`
- Windows: `powershell` NotifyIcon balloon notification
- Silently degrades if the notification tool is not installed.

## Running as a system service

### Linux (systemd)

```sh
cp services/corky-watch.service ~/.config/systemd/user/
# Edit WorkingDirectory in the unit file to match your setup
systemctl --user enable --now corky-watch
journalctl --user -u corky-watch -f   # view logs
```

### macOS (launchd)

```sh
cp services/com.corky.watch.plist ~/Library/LaunchAgents/
# Edit WorkingDirectory in the plist to match your setup
launchctl load ~/Library/LaunchAgents/com.corky.watch.plist
tail -f /tmp/corky-watch.log          # view logs
```

## Signals

SIGTERM and SIGINT trigger a clean shutdown — finish the current poll, then exit.
